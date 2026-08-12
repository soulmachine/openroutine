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

    let out = env.run_ok(&["init"]);

    assert!(out.contains("config"), "it should say what it made: {out}");
    assert!(env.config_path().exists(), "a config");
    assert!(
        std::fs::read_dir(env.project_dir()).unwrap().count() == 0,
        "init registers nothing and writes no task anywhere: that is `add`'s job"
    );

    // The sample it prints is the thing a new user copies, so it has to be a
    // task that actually registers.
    let sample = out
        .split_once("---\n")
        .map(|(_, rest)| format!("---\n{}", rest.split("\nThen:").next().unwrap_or("")))
        .expect("init should print a sample task");
    let file = env.path().join("hello.md");
    std::fs::write(&file, sample).unwrap();

    env.run_ok(&["add", file.to_str().unwrap()]);
    let listed = env.run_ok(&["list"]);
    assert!(
        !listed.to_lowercase().contains("broken"),
        "the printed sample must be usable as written:\n{listed}"
    );
}

#[test]
fn init_does_not_overwrite_an_existing_config() {
    let env = TestEnv::new();
    env.write_config();
    let before = std::fs::read_to_string(env.config_path()).unwrap();

    let output = env.run(&["init"]);

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
    .unwrap()
    .watching_config(env.config_path());
    daemon.reload().await.unwrap();
    clock.set(support::at("2026-08-11T01:00:00Z"));
    daemon.tick().await.unwrap();
    daemon.wait_for_running().await;

    let out = env.run_ok(&["logs", "hourly"]);

    assert!(out.contains("stub agent ran"), "{out}");
    assert!(out.contains("hourly"), "the header comes too: {out}");
}

#[test]
fn logs_for_a_task_that_never_ran_says_so() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config();

    let out = env.run_ok(&["logs", "nightly"]);

    assert!(out.to_lowercase().contains("no runs"), "{out}");
}

// --- naming -------------------------------------------------------------

#[test]
fn a_bare_task_name_works_when_it_is_unambiguous() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config();

    let out = env.run_ok(&["run", "nightly", "--dry-run"]);

    assert!(out.contains("nightly"), "{out}");
}

#[test]
fn a_second_file_claiming_a_taken_name_is_broken_and_the_first_keeps_it() {
    let env = TestEnv::new();
    let first = env.write_task("nightly", NIGHTLY);
    // Same `name:`, different file: registration order decides.
    let second = env.write_unregistered_task("later", NIGHTLY);
    std::fs::write(&second, std::fs::read_to_string(&first).unwrap()).unwrap();
    env.register(&second);
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(
        out.contains("broken") && out.contains("already taken"),
        "the later file should say whose name it wanted:\n{out}"
    );
    // The name still resolves — to the file that claimed it first, which is
    // still usable rather than broken.
    let dry = env.run_ok(&["run", "nightly", "--dry-run"]);
    assert!(
        dry.contains("Nightly TODO/FIXME triage"),
        "the first file keeps the name:\n{dry}"
    );
    let _ = &first;
    // And the loser is still reachable by path, which is how it gets fixed.
    let by_path = env.run_ok(&["remove", &second.display().to_string()]);
    assert!(by_path.contains("Unregistered"), "{by_path}");
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
        "---\ndescription: Nightly\ncron: \"0 2 * * *\"\nagent: stub\ncwd: sub\nenv:\n  API_TOKEN: secret-value\n---\n\nping\n",
    );
    env.write_config();

    let out = env.run_ok(&["run", "nightly", "--dry-run"]);

    assert!(out.contains("sub"), "the working directory: {out}");
    assert!(out.contains("15m"), "the machine-wide idle timeout: {out}");
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

    // No cron and no at: it runs once, when the daemon takes it in.
    let asap = env.run_ok(&["run", "ondemand", "--dry-run"]);
    assert!(asap.to_lowercase().contains("once"), "{asap}");
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

    let out = env.run_ok(&["init"]);

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

/// The check exists to predict what a service-managed Daemon will find, and
/// it is only ever run from a terminal — so it must not answer with what the
/// terminal can reach. A program on the caller's `PATH` and nowhere a login
/// profile would put it is exactly the shape that reads "ready" in `list`
/// and then fails at 2am under launchd.
#[test]
fn an_agent_only_the_calling_shell_can_reach_still_warns() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config_with_agent("blindspot-agent --run {prompt}");
    let bin = env.write_program_in("interactive-bin", "blindspot-agent");

    // A login shell with no profile of its own, so the only way to reach the
    // program is the PATH we hand the caller — which a Run will not inherit.
    let out = env.run_with_env(
        &["list"],
        &[
            ("PATH", &format!("/usr/bin:/bin:{}", bin.display())),
            ("SHELL", "/bin/sh"),
            ("HOME", &env.path().display().to_string()),
        ],
    );
    let out = String::from_utf8(out.stdout).unwrap();

    assert!(
        out.to_lowercase().contains("warning"),
        "a program only the calling shell can reach is not findable by a Run:\n{out}"
    );
    assert!(
        out.contains("service manager"),
        "and the warning should name the PATH that is missing it, not just \
         claim it is absent when `command -v` finds it:\n{out}"
    );
}

/// The other half of the same fix: seeding the sampled `PATH` must not start
/// warning about programs a service manager can perfectly well find.
#[test]
fn an_agent_on_the_service_managers_own_path_says_nothing() {
    let env = TestEnv::new();
    env.write_task("nightly", NIGHTLY);
    env.write_config_with_agent("sh -c {prompt}");

    let out = env.run_ok(&["list"]);

    assert!(
        !out.to_lowercase().contains("warning"),
        "/bin/sh is on the barest PATH there is; nothing to warn about:\n{out}"
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
