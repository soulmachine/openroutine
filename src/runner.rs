//! Turning an Agent command template plus a prompt into an argv.
//!
//! The prompt is substituted for `{prompt}` as a single argument — the
//! template is split shell-lessly, so a prompt can never inject flags or
//! shell syntax. A template without the placeholder gets the prompt on stdin.

use anyhow::{Result, bail};

pub const PROMPT_PLACEHOLDER: &str = "{prompt}";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentCommand {
    pub program: String,
    pub args: Vec<String>,
    /// True when the template had no `{prompt}`, so the body is piped instead.
    pub prompt_on_stdin: bool,
}

/// Checks a template can produce a command at all, without needing a prompt.
/// Called when a Task is scanned, so a malformed template makes the Task
/// Broken and visible rather than failing invisibly at fire time.
pub fn validate_template(template: &str) -> Result<(), String> {
    build_command(template, "").map(|_| ()).map_err(|error| {
        format!(
            "agent command template is unusable: {}",
            error.to_string().trim_end_matches('.')
        )
    })
}

/// Builds the command for one Run from the Agent's template.
pub fn build_command(template: &str, prompt: &str) -> Result<AgentCommand> {
    let tokens = split_tokens(template)?;
    let Some((program, rest)) = tokens.split_first() else {
        bail!("agent command template is empty");
    };

    let mut substituted = false;
    let args = rest
        .iter()
        .map(|token| {
            if token.contains(PROMPT_PLACEHOLDER) {
                substituted = true;
                token.replace(PROMPT_PLACEHOLDER, prompt)
            } else {
                token.clone()
            }
        })
        .collect();

    // A placeholder in the program position would mean executing the prompt.
    if program.contains(PROMPT_PLACEHOLDER) {
        bail!("agent command template must not use {PROMPT_PLACEHOLDER} as the program");
    }

    Ok(AgentCommand {
        program: program.clone(),
        args,
        prompt_on_stdin: !substituted,
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
