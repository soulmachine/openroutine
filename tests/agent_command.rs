//! The Agent contract: a command template plus a prompt becomes an argv.
//! Pure — no process is spawned here.

use openroutine::runner::build_command;

#[test]
fn the_prompt_replaces_the_placeholder_as_one_argument() {
    let command = build_command("claude -p {prompt}", "do the thing").unwrap();

    assert_eq!(command.program, "claude");
    assert_eq!(command.args, vec!["-p", "do the thing"]);
    assert!(!command.prompt_on_stdin);
}

#[test]
fn a_template_without_the_placeholder_pipes_instead() {
    let command = build_command("codex exec", "do the thing").unwrap();

    assert_eq!(command.args, vec!["exec"]);
    assert!(command.prompt_on_stdin);
}

#[test]
fn a_placeholder_embedded_in_a_flag_stays_one_argument() {
    let command = build_command("agent --prompt={prompt}", "hello").unwrap();

    assert_eq!(command.args, vec!["--prompt=hello"]);
}

#[test]
fn no_shell_is_involved_so_prompt_metacharacters_are_inert() {
    let nasty = "; rm -rf / && echo $(whoami) `id` | tee /tmp/x";

    let command = build_command("claude -p {prompt}", nasty).unwrap();

    assert_eq!(
        command.args,
        vec!["-p", nasty],
        "the prompt is exactly one argument, whatever it contains"
    );
}

#[test]
fn a_prompt_that_looks_like_flags_is_still_one_argument() {
    let command = build_command("claude -p {prompt}", "--dangerously-skip-permissions").unwrap();

    assert_eq!(command.args, vec!["-p", "--dangerously-skip-permissions"]);
}

#[test]
fn a_multiline_prompt_survives_intact() {
    let command = build_command("claude -p {prompt}", "line one\n\nline two").unwrap();

    assert_eq!(command.args, vec!["-p", "line one\n\nline two"]);
}

#[test]
fn quoted_segments_of_the_template_are_single_tokens() {
    let command = build_command(
        r#""/opt/my agents/claude" --flag 'two words' {prompt}"#,
        "p",
    )
    .unwrap();

    assert_eq!(command.program, "/opt/my agents/claude");
    assert_eq!(command.args, vec!["--flag", "two words", "p"]);
}

#[test]
fn an_unterminated_quote_is_rejected() {
    assert!(build_command("claude -p 'unclosed {prompt}", "p").is_err());
}

#[test]
fn an_empty_template_is_rejected() {
    assert!(build_command("   ", "p").is_err());
}

#[test]
fn the_placeholder_may_not_be_the_program() {
    assert!(
        build_command("{prompt} --go", "rm -rf /").is_err(),
        "executing the prompt itself must never be possible"
    );
}
