//! The rest of the terminal experience: starting out, looking in, and
//! seeing what a Task would do before it does it.

mod support;

use support::TestEnv;

const NIGHTLY: &str = r#"---
description: Nightly TODO/FIXME triage
cron: "0 2 * * *"
agent: stub
---

Review all open TODO and FIXME comments.
"#;

// --- init ---------------------------------------------------------------

#[test]
fn init_produces_a_setup_that_works_immediately() {
    let env = TestEnv::new();

    let out = env.run_ok(&["init", env.project_dir().to_str().unwrap()]);

    assert!(out.contains("config"), "it should say what it made: {out}");
    assert!(env.config_path().exists(), "a config");
    let sample: Vec<_> = std::fs::read_dir(env.project_dir())
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().to_string_lossy().to_string();
            name.ends_with(".cron.md").then_some(name)
        })
        .collect();
    assert_eq!(sample.len(), 1, "and a task to look at: {sample:?}");

    // The whole point: what it wrote is usable without editing anything.
    let listed = env.run_ok(&["list"]);
    assert!(
        !listed.to_lowercase().contains("broken"),
        "the sample task must be usable as written:\n{listed}"
    );
}

#[test]
fn init_does_not_overwrite_an_existing_config() {
    let env = TestEnv::new();
    env.write_config();
    let before = std::fs::read_to_string(env.config_path()).unwrap();

    let output = env.run(&["init", env.project_dir().to_str().unwrap()]);

    assert!(
        !output.status.success(),
        "it must refuse rather than clobber"
    );
    assert_eq!(std::fs::read_to_string(env.config_path()).unwrap(), before);
}

// --- status -------------------------------------------------------------

#[test]
fn status_reports_a_stopped_daemon_and_what_is_configured() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_task("broken", "---\ncron: \"nope\"\n---\n\nx\n");
    env.write_config();

    let out = env.run_ok(&["status"]);

    assert!(
        out.to_lowercase().contains("not running"),
        "with no daemon it should say so:\n{out}"
    );
    assert!(out.contains("1 ready"), "{out}");
    assert!(out.contains("1 broken"), "{out}");
    assert!(
        out.contains(&env.state_dir().display().to_string()),
        "and where things live:\n{out}"
    );
}

// --- logs ---------------------------------------------------------------

#[tokio::test]
async fn logs_prints_the_latest_run() {
    let env = TestEnv::new();
    env.write_task(
        "hourly",
        "---\ndescription: Hourly\ncron: \"@hourly\"\nagent: stub\njitter: 0\n---\n\nping\n",
    );
    env.write_config();

    let clock = openroutine::clock::ManualClock::new(support::at("2026-08-11T00:30:00Z"));
    let mut daemon = openroutine::daemon::Daemon::with_zone(
        env.load_config(),
        std::sync::Arc::new(clock.clone()),
        chrono_tz::UTC,
    )
    .unwrap();
    daemon.reload().await.unwrap();
    clock.set(support::at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let out = env.run_ok(&["logs", "proj/hourly"]);

    assert!(out.contains("stub agent ran"), "{out}");
    assert!(out.contains("proj/hourly"), "the header comes too: {out}");
}

#[test]
fn logs_for_a_task_that_never_ran_says_so() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config();

    let out = env.run_ok(&["logs", "proj/nightly"]);

    assert!(out.to_lowercase().contains("no runs"), "{out}");
}

// --- naming -------------------------------------------------------------

#[test]
fn a_bare_task_name_works_when_it_is_unambiguous() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config();

    let out = env.run_ok(&["run", "nightly", "--dry-run"]);

    assert!(out.contains("proj/nightly"), "{out}");
}

#[test]
fn an_ambiguous_bare_name_lists_the_candidates() {
    let env = TestEnv::new();
    let other = env.add_project("other");
    env.write_task("nightly", NIGHTLY);
    std::fs::write(other.join("nightly.cron.md"), NIGHTLY).unwrap();
    env.write_config_with_projects();

    let output = env.run(&["run", "nightly", "--dry-run"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("proj/nightly"), "{stderr}");
    assert!(stderr.contains("other/nightly"), "{stderr}");
}

#[test]
fn a_name_that_matches_nothing_says_so() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config();

    let output = env.run(&["run", "ghost", "--dry-run"]);

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("ghost"),
        "the error should quote what was asked for"
    );
}

// --- dry run ------------------------------------------------------------

#[test]
fn a_dry_run_shows_the_command_without_running_anything() {
    let env = TestEnv::new();
    let marker = env.path().join("should-not-exist");
    env.write_task(
        "nightly",
        &format!(
            "---\ndescription: Nightly\ncron: \"0 2 * * *\"\nagent: stub\n---\n\n; touch {}\n",
            marker.display()
        ),
    );
    env.write_config();

    let out = env.run_ok(&["run", "nightly", "--dry-run"]);

    assert!(!marker.exists(), "a dry run spawns nothing");
    assert!(out.contains("--run"), "the argv is shown: {out}");
    assert!(
        out.contains("; touch"),
        "including the prompt, exactly as it will be passed: {out}"
    );
    assert!(
        out.contains("-l") && out.contains("exec") && out.contains("$@"),
        "and the login shell that will carry it:\n{out}"
    );
}

#[test]
fn a_dry_run_shows_where_and_under_what_limits_it_would_run() {
    let env = TestEnv::new();
    std::fs::create_dir_all(env.project_dir().join("sub")).unwrap();
    env.write_task(
        "nightly",
        "---\ndescription: Nightly\ncron: \"0 2 * * *\"\nagent: stub\ncwd: sub\ntimeout: 30m\nenv:\n  API_TOKEN: secret-value\n---\n\nping\n",
    );
    env.write_config();

    let out = env.run_ok(&["run", "nightly", "--dry-run"]);

    assert!(out.contains("sub"), "the working directory: {out}");
    assert!(
        out.contains("30m") || out.contains("1800"),
        "the timeout: {out}"
    );
    assert!(out.contains("API_TOKEN"), "which env it sets: {out}");
    assert!(
        !out.contains("secret-value"),
        "but never the values — a dry run is something you paste into a bug report:\n{out}"
    );
}

#[test]
fn a_dry_run_predicts_when_each_kind_of_task_would_next_fire() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_task(
        "once",
        "---\ndescription: Once\nat: \"2026-12-25T09:00:00Z\"\nagent: stub\n---\n\nping\n",
    );
    env.write_task(
        "ondemand",
        "---\ndescription: On demand\nagent: stub\n---\n\nping\n",
    );
    env.write_config();

    let cron = env.run_ok(&["run", "nightly", "--dry-run"]);
    assert!(cron.contains("T02:0"), "a jittered next fire: {cron}");

    let once = env.run_ok(&["run", "once", "--dry-run"]);
    assert!(once.contains("2026-12-25"), "{once}");

    let manual = env.run_ok(&["run", "ondemand", "--dry-run"]);
    assert!(manual.to_lowercase().contains("no schedule"), "{manual}");
}

#[test]
fn a_dry_run_of_a_broken_task_explains_instead_of_pretending() {
    let env = TestEnv::new();
    env.write_task(
        "typo",
        "---\ndescription: Bad\ncron: \"0 25 * * *\"\nagent: stub\n---\n\nping\n",
    );
    env.write_config();

    let output = env.run(&["run", "typo", "--dry-run"]);

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("0 25 * * *"), "{stderr}");
}

// --- install ------------------------------------------------------------

#[test]
fn install_can_show_what_it_would_register_without_registering_it() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config();

    let out = env.run_ok(&["install", "--print"]);

    assert!(out.contains("openroutine"), "{out}");
    assert!(
        out.contains("serve") && out.contains(&env.config_path().display().to_string()),
        "the service must start this daemon with this config:\n{out}"
    );
    assert!(
        out.contains("SHELL"),
        "and carry a SHELL, since a service manager provides none — which is \
         what the login-shell environment depends on:\n{out}"
    );
    assert!(
        out.to_lowercase().contains("keepalive") || out.contains("Restart=always"),
        "and ask to be restarted if it stops:\n{out}"
    );
}

#[test]
fn install_print_writes_nothing_at_all() {
    let env = TestEnv::new();
    env.write_config();
    let agents = env.path().join("Library/LaunchAgents");

    env.run_ok(&["install", "--print"]);

    assert!(!agents.exists(), "printing is not installing");
}

#[test]
fn init_points_at_the_daemon_too() {
    let env = TestEnv::new();

    let out = env.run_ok(&["init", env.project_dir().to_str().unwrap()]);

    assert!(
        out.contains("serve"),
        "the next thing a new user wants is a running daemon; init should say so:\n{out}"
    );
}

#[test]
fn an_agent_whose_program_is_missing_warns_without_breaking_the_task() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config_with_agent("/definitely/not/here --run {prompt}");

    let out = env.run_ok(&["list"]);

    assert!(
        !out.to_lowercase().contains("broken"),
        "the program might still resolve at run time; this is a warning, not a fault:\n{out}"
    );
    assert!(out.to_lowercase().contains("warning"), "{out}");
    assert!(
        out.contains("/definitely/not/here"),
        "and it should name what it could not find:\n{out}"
    );
}

#[test]
fn an_agent_named_by_a_command_that_is_not_installed_warns() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config_with_agent("definitely-not-a-real-command-xyz --run {prompt}");

    let out = env.run_ok(&["list"]);

    assert!(out.to_lowercase().contains("warning"), "{out}");
    assert!(out.contains("definitely-not-a-real-command-xyz"), "{out}");
}

#[test]
fn an_agent_that_is_installed_says_nothing() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config(); // the stub agent, an absolute path that exists

    let out = env.run_ok(&["list"]);

    assert!(
        !out.to_lowercase().contains("cannot find"),
        "no complaint about an agent that is right there:\n{out}"
    );
}
