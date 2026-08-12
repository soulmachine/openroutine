//! Task definitions — the `.cron.md` file is the whole definition.
//! Pure parsing: text in, definition out, no I/O.

use openroutine::task::TaskDefinition;

const NIGHTLY: &str = r#"---
description: Nightly TODO/FIXME triage
cron: "0 2 * * *"
agent: claude
timeout: 30m
---

Review all open TODO and FIXME comments in this repository.
Summarize everything else in reports/todo-digest.md.
"#;

#[test]
fn frontmatter_supplies_metadata_and_the_body_is_the_prompt() {
    let definition = TaskDefinition::parse(NIGHTLY).unwrap();

    assert_eq!(definition.description, "Nightly TODO/FIXME triage");
    assert_eq!(definition.schedule.expression(), "0 2 * * *");
    assert_eq!(definition.agent.as_deref(), Some("claude"));
    assert_eq!(
        definition.prompt,
        "Review all open TODO and FIXME comments in this repository.\n\
         Summarize everything else in reports/todo-digest.md."
    );
}

#[test]
fn the_prompt_keeps_its_internal_shape() {
    let source =
        "---\ndescription: d\ncron: \"@daily\"\n---\n\nFirst para.\n\n  indented line\n\nLast.\n";

    let definition = TaskDefinition::parse(source).unwrap();

    assert_eq!(definition.prompt, "First para.\n\n  indented line\n\nLast.");
}

#[test]
fn agent_is_optional_in_the_file() {
    let source = "---\ndescription: d\ncron: \"@daily\"\n---\n\nbody\n";

    let definition = TaskDefinition::parse(source).unwrap();

    assert_eq!(definition.agent, None);
}

#[test]
fn a_file_without_frontmatter_is_rejected() {
    let err = TaskDefinition::parse("just a prompt, no frontmatter\n").unwrap_err();

    assert!(
        err.to_string().to_lowercase().contains("frontmatter"),
        "error should name the problem, got: {err}"
    );
}
