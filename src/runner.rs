//! Turning an Agent command template plus a Task's parameters into an argv.
//!
//! Two rules hold everywhere here. Values are substituted as whole arguments,
//! never spliced into a shell string, so nothing a Task can write becomes
//! syntax. And the command is handed to a login shell only through `$0`/`$@`,
//! so the shell supplies the environment without ever parsing our arguments.

use anyhow::{Result, bail};
use std::collections::BTreeSet;

/// Placeholders a template may use.
pub const PROMPT: &str = "prompt";
pub const MODEL: &str = "model";
pub const PERMISSION_MODE: &str = "permission_mode";
const KNOWN: [&str; 3] = [PROMPT, MODEL, PERMISSION_MODE];

/// The fallback shell when the environment names none.
const FALLBACK_SHELL: &str = "/bin/sh";
/// Runs the command with our arguments as real argv, after the login profile.
const EXEC_ARGV: &str = r#"exec "$0" "$@""#;
/// Applies environment overrides after the profile, so they actually win.
const ENV: &str = "/usr/bin/env";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    pub program: String,
    pub args: Vec<String>,
    /// True when the template had no `{prompt}`, so the body is piped instead.
    pub prompt_on_stdin: bool,
}

/// What a particular Task contributes to its command line.
#[derive(Debug, Default, Clone)]
pub struct AgentParams<'a> {
    pub prompt: &'a str,
    pub model: Option<&'a str>,
    pub permission_mode: Option<&'a str>,
}

impl AgentParams<'_> {
    fn value(&self, placeholder: &str) -> Option<&str> {
        match placeholder {
            PROMPT => Some(self.prompt),
            MODEL => self.model,
            PERMISSION_MODE => self.permission_mode,
            _ => None,
        }
    }
}

/// Builds the command for one Run.
///
/// A placeholder whose value is unset takes its argument with it, and the
/// flag in front of it too — otherwise omitting `model:` would leave a bare
/// `--model` for the Agent to choke on.
pub fn build_command(template: &str, params: &AgentParams) -> Result<AgentCommand> {
    let tokens = split_tokens(template)?;
    let Some((program, rest)) = tokens.split_first() else {
        bail!("agent command template is empty");
    };
    if placeholders_in(program).next().is_some() {
        bail!("agent command template must not use a placeholder as the program");
    }

    let mut args: Vec<String> = Vec::with_capacity(rest.len());
    let mut prompt_used = false;

    for token in rest {
        let mut names = placeholders_in(token).peekable();
        if names.peek().is_none() {
            args.push(token.clone());
            continue;
        }

        let names: Vec<&str> = names.collect();
        if let Some(unset) = names.iter().find(|name| params.value(name).is_none()) {
            tracing::debug!("dropping {token:?}: no value for {{{unset}}}");
            // Take the flag it belonged to, if that is what precedes it.
            if args
                .last()
                .is_some_and(|previous| previous.starts_with('-') && !previous.contains('{'))
            {
                args.pop();
            }
            continue;
        }

        let mut filled = token.clone();
        for name in names {
            let value = params.value(name).unwrap_or_default();
            filled = filled.replace(&format!("{{{name}}}"), value);
            prompt_used |= name == PROMPT;
        }
        args.push(filled);
    }

    Ok(AgentCommand {
        program: program.clone(),
        args,
        prompt_on_stdin: !prompt_used,
    })
}

/// Wraps a command so it runs under the user's login shell, with `overrides`
/// applied afterwards.
///
/// The shell reads the profile — so `PATH`, shims, and API keys match a
/// terminal — then `exec`s the real command with our arguments untouched.
pub fn login_shell_command(command: AgentCommand, overrides: &[(String, String)]) -> AgentCommand {
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| FALLBACK_SHELL.to_string());

    let mut args = vec!["-l".to_string(), "-c".to_string(), EXEC_ARGV.to_string()];
    let assignments: Vec<String> = overrides
        .iter()
        .filter(|(name, _)| is_usable_env_name(name))
        .map(|(name, value)| format!("{name}={value}"))
        .collect();

    if !assignments.is_empty() {
        // `env` runs after the profile, so an override beats whatever a
        // dotfile set. Values travel as argv, never through the shell — and
        // `--` stops a variable name from being read as an option to `env`,
        // which on GNU systems would otherwise let `--split-string=...` run
        // a command of the Task author's choosing.
        args.push(ENV.to_string());
        args.push("--".to_string());
        args.extend(assignments);
    }
    args.push(command.program);
    args.extend(command.args);

    AgentCommand {
        program: shell,
        args,
        prompt_on_stdin: command.prompt_on_stdin,
    }
}

/// Checks a template can produce a command at all, without needing values.
/// Called when a Task is scanned, so a malformed template makes the Task
/// Broken and visible rather than failing invisibly at fire time.
pub fn validate_template(template: &str) -> Result<(), String> {
    build_command(template, &AgentParams::default())
        .map(|_| ())
        .map_err(|error| {
            format!(
                "agent command template is unusable: {}",
                error.to_string().trim_end_matches('.')
            )
        })
}

/// Whether a name can safely be exported.
///
/// `--` already stops `env` reading a name as an option; this refuses the
/// shapes that would silently mean something other than what was written.
pub fn is_usable_env_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('=')
        && !name.contains('\0')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit())
}

/// Rejects placeholders this build cannot fill.
///
/// This is a config-level check rather than a per-Task one: a typo in a
/// shared Agent template is a mistake in the config itself, and every Task
/// pointing at that Agent would be equally broken by it.
pub fn check_placeholders(template: &str) -> Result<(), String> {
    // Read the raw text rather than tokens: a template that also fails to
    // tokenise is a per-Task problem (it becomes Broken), and reporting it
    // here instead would stop every unrelated Task in the config too.
    //
    // Anything braced counts here, not just well-formed names — `{promt}` and
    // `{permission-mode}` are both typos the author wants told about, and
    // neither would ever be filled in.
    for name in braced_spans(template) {
        if !KNOWN.contains(&name) {
            return Err(format!(
                "unknown placeholder {{{name}}}; this build understands \
                 {{prompt}}, {{model}}, and {{permission_mode}}"
            ));
        }
    }
    Ok(())
}

/// Every placeholder a template mentions.
pub fn template_placeholders(template: &str) -> BTreeSet<String> {
    split_tokens(template)
        .unwrap_or_default()
        .iter()
        .flat_map(|token| placeholders_in(token))
        .map(str::to_string)
        .collect()
}

/// Everything written between braces, well-formed or not.
fn braced_spans(text: &str) -> impl Iterator<Item = &str> {
    text.split('{')
        .skip(1)
        .filter_map(|rest| rest.split_once('}').map(|(name, _)| name))
}

/// The `{name}` placeholders inside one token.
fn placeholders_in(token: &str) -> impl Iterator<Item = &str> {
    token.split('{').skip(1).filter_map(|rest| {
        let name = rest.split_once('}')?.0;
        (!name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            .then_some(name)
    })
}

/// Splits a command template into tokens, honouring single and double quotes.
/// No shell is involved: no expansion, no globbing, no operators.
fn split_tokens(template: &str) -> Result<Vec<String>> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut has_token = false;
    let mut quote: Option<char> = None;
    let mut chars = template.chars();

    while let Some(character) = chars.next() {
        match (quote, character) {
            (Some(open), c) if c == open => quote = None,
            (Some(_), c) => current.push(c),
            (None, '\'') | (None, '"') => {
                quote = Some(character);
                has_token = true;
            }
            (None, '\\') => {
                if let Some(escaped) = chars.next() {
                    current.push(escaped);
                    has_token = true;
                }
            }
            (None, c) if c.is_whitespace() => {
                if has_token {
                    tokens.push(std::mem::take(&mut current));
                    has_token = false;
                }
            }
            (None, c) => {
                current.push(c);
                has_token = true;
            }
        }
    }

    if quote.is_some() {
        bail!("agent command template has an unterminated quote");
    }
    if has_token {
        tokens.push(current);
    }
    Ok(tokens)
}

/// The `PATH` a Run will actually see.
///
/// Runs launch through a login shell, so the Daemon's own `PATH` is the
/// wrong thing to check against — under a service manager it is nearly
/// empty, which is the very problem the login shell solves. This asks the
/// shell once and remembers the answer.
fn login_path() -> Option<&'static str> {
    static PATH: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    PATH.get_or_init(|| {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|shell| !shell.is_empty())
            .unwrap_or_else(|| FALLBACK_SHELL.to_string());
        let output = std::process::Command::new(shell)
            .args(["-l", "-c", "printf %s \"$PATH\""])
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
            .filter(|path| !path.is_empty())
    })
    .as_deref()
}

/// Whether the Agent's program can be found the way a Run would find it.
///
/// Advisory only: a profile may put something on `PATH` conditionally, so
/// not finding it now is a reason to warn, never a reason to refuse.
pub fn program_is_findable(program: &str) -> bool {
    let candidate = std::path::Path::new(program);
    if candidate.is_absolute() || program.contains('/') {
        return is_executable(candidate);
    }

    let Some(path) = login_path() else {
        // Nothing to check against; say nothing rather than warn wrongly.
        return true;
    };
    path.split(':')
        .filter(|entry| !entry.is_empty())
        .any(|entry| is_executable(&std::path::Path::new(entry).join(program)))
}

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}
