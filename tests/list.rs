//! `openroutine list` at the process boundary: every Task, its health, and
//! why — read from disk, with no Daemon running anywhere.

mod support;

use support::TestEnv;

const NIGHTLY: &str = r#"---
description: Nightly TODO/FIXME triage
cron: "0 2 * * *"
agent: stub
---

Review all open TODO and FIXME comments.
"#;

#[test]
fn a_ready_task_shows_its_id_description_schedule_agent_and_next_fire() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("proj/todo-digest"), "{out}");
    assert!(out.contains("Nightly TODO/FIXME triage"), "{out}");
    assert!(out.contains("0 2 * * *"), "{out}");
    assert!(out.contains("stub"), "{out}");
    // The next Tick of `0 2 * * *` is 02:00 in the host's own zone.
    assert!(
        out.contains("T02:00:00"),
        "expected the next Tick at 02:00 local, got:\n{out}"
    );
}

#[test]
fn listing_needs_no_daemon_and_no_prior_state() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config();

    assert!(
        !env.state_file().exists(),
        "precondition: nothing has run yet"
    );

    let out = env.run_ok(&["list"]);

    assert!(out.contains("proj/todo-digest"), "{out}");
    assert!(
        !env.state_file().exists(),
        "a read must not create machine state"
    );
}

#[test]
fn a_task_missing_its_description_is_broken_and_says_which_field() {
    let env = TestEnv::new();
    env.write_task(
        "nameless",
        "---\ncron: \"0 2 * * *\"\nagent: stub\n---\n\nbody\n",
    );
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("proj/nameless"), "{out}");
    assert!(out.to_lowercase().contains("broken"), "{out}");
    assert!(
        out.contains("description"),
        "the error should name the missing field, got:\n{out}"
    );
}

#[test]
fn an_invalid_cron_expression_is_broken_with_the_parse_error_verbatim() {
    let env = TestEnv::new();
    env.write_task(
        "typo",
        "---\ndescription: Typo'd schedule\ncron: \"0 25 * * *\"\nagent: stub\n---\n\nbody\n",
    );
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("proj/typo"), "{out}");
    assert!(out.to_lowercase().contains("broken"), "{out}");
    assert!(
        out.contains("0 25 * * *"),
        "the offending expression should be quoted back, got:\n{out}"
    );
}

#[test]
fn a_task_naming_an_unknown_agent_is_broken() {
    let env = TestEnv::new();
    env.write_task(
        "wrong-agent",
        "---\ndescription: Points at nothing\ncron: \"@daily\"\nagent: ghost\n---\n\nbody\n",
    );
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("proj/wrong-agent"), "{out}");
    assert!(out.to_lowercase().contains("broken"), "{out}");
    assert!(
        out.contains("ghost"),
        "the error should name the agent, got:\n{out}"
    );
}

#[test]
fn a_task_with_no_agent_and_no_default_is_broken() {
    let env = TestEnv::new();
    env.write_task(
        "agentless",
        "---\ndescription: No agent anywhere\ncron: \"@daily\"\n---\n\nbody\n",
    );
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.to_lowercase().contains("broken"), "{out}");
    assert!(out.contains("agent"), "{out}");
}

#[test]
fn a_default_agent_in_config_covers_tasks_that_omit_one() {
    let env = TestEnv::new();
    env.write_task(
        "agentless",
        "---\ndescription: Relies on the default\ncron: \"@daily\"\n---\n\nbody\n",
    );
    env.write_config_with_default_agent();

    let out = env.run_ok(&["list"]);

    assert!(
        !out.to_lowercase().contains("broken"),
        "the config default should resolve it, got:\n{out}"
    );
    assert!(out.contains("stub"), "{out}");
}

#[test]
fn a_file_without_frontmatter_is_broken_rather_than_ignored() {
    let env = TestEnv::new();
    env.write_task("bare", "just a prompt, no frontmatter\n");
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(
        out.contains("proj/bare"),
        "a task file must never vanish silently, got:\n{out}"
    );
    assert!(out.to_lowercase().contains("broken"), "{out}");
    assert!(out.to_lowercase().contains("frontmatter"), "{out}");
}

#[test]
fn an_unknown_frontmatter_key_warns_while_the_task_stays_ready() {
    let env = TestEnv::new();
    env.write_task(
        "odd",
        "---\ndescription: Has a key from the future\ncron: \"@daily\"\nagent: stub\nretries: 3\n---\n\nbody\n",
    );
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("proj/odd"), "{out}");
    assert!(
        !out.to_lowercase().contains("broken"),
        "an unknown key must not break the task, got:\n{out}"
    );
    assert!(out.to_lowercase().contains("warning"), "{out}");
    assert!(
        out.contains("retries"),
        "the warning should name the key, got:\n{out}"
    );
}

#[test]
fn broken_tasks_sit_alongside_healthy_ones() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_task(
        "typo",
        "---\ndescription: Bad\ncron: \"nope\"\nagent: stub\n---\n\nbody\n",
    );
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("proj/todo-digest"), "{out}");
    assert!(out.contains("Nightly TODO/FIXME triage"), "{out}");
    assert!(out.contains("proj/typo"), "{out}");
    assert!(
        out.contains("1 broken"),
        "the summary should count the broken one, got:\n{out}"
    );
}

#[test]
fn a_task_whose_agent_template_cannot_be_parsed_is_broken() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config_with_agent("stub --run 'unterminated {prompt}");

    let out = env.run_ok(&["list"]);

    assert!(out.to_lowercase().contains("broken"), "{out}");
    assert!(
        out.contains("template"),
        "the error should point at the agent template, got:\n{out}"
    );
}

#[test]
fn an_agent_template_that_would_execute_the_prompt_is_broken() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config_with_agent("{prompt} --go");

    let out = env.run_ok(&["list"]);

    assert!(
        out.to_lowercase().contains("broken"),
        "running the prompt as a program must never be reachable, got:\n{out}"
    );
}

#[test]
fn an_empty_agent_template_is_broken() {
    let env = TestEnv::new();
    env.write_task("todo-digest", NIGHTLY);
    env.write_config_with_agent("   ");

    let out = env.run_ok(&["list"]);

    assert!(out.to_lowercase().contains("broken"), "{out}");
}

#[test]
fn a_broken_task_keeps_the_description_its_author_wrote() {
    let env = TestEnv::new();
    env.write_task(
        "typo",
        "---\ndescription: Fat-fingered schedule\ncron: \"0 25 * * *\"\nagent: stub\n---\n\nbody\n",
    );
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(
        out.contains("Fat-fingered schedule"),
        "a Broken Task should still be recognisable by name, got:\n{out}"
    );
    assert!(out.contains("broken"), "{out}");
}

#[test]
fn a_schema_key_this_build_ignores_says_so_rather_than_calling_it_unknown() {
    let env = TestEnv::new();
    // `tz` is in the v1 schema and reserved; this build does not act on it.
    env.write_task(
        "zoned",
        "---\ndescription: Wants its own zone\ncron: \"@daily\"\nagent: stub\ntz: Europe/Berlin\n---\n\nbody\n",
    );
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(out.contains("tz"), "{out}");
    assert!(
        !out.contains("unknown frontmatter key: tz"),
        "`tz` is part of the schema, not an unknown key — saying otherwise misleads:\n{out}"
    );
    assert!(
        out.to_lowercase().contains("does not act on it yet"),
        "the warning should admit the field is unimplemented, got:\n{out}"
    );
}

#[test]
fn an_empty_project_says_so_plainly() {
    let env = TestEnv::new();
    env.write_config();

    let out = env.run_ok(&["list"]);

    assert!(
        out.to_lowercase().contains("no tasks"),
        "expected a plain empty message, got:\n{out}"
    );
}
