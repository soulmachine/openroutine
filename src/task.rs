//! The Task definition: one `.cron.md` file, frontmatter plus prompt body.
//!
//! The file is the complete definition — there is no second place to look.

use crate::schedule::{Schedule, ScheduleError};
use serde::Deserialize;
use std::collections::BTreeMap;

const FENCE: &str = "---";

#[derive(Debug, thiserror::Error)]
pub enum TaskError {
    #[error("no frontmatter: a task file must open with a `---` fenced block")]
    MissingFrontmatter,
    #[error("unterminated frontmatter: the opening `---` has no closing `---`")]
    UnterminatedFrontmatter,
    #[error("invalid frontmatter: {0}")]
    InvalidFrontmatter(String),
    #[error(transparent)]
    Schedule(#[from] ScheduleError),
}

/// Frontmatter keys the v1 schema defines but this build does not act on
/// yet. Warned about specifically: telling someone their `disabled: true` is
/// an "unknown key" would be a lie, and saying nothing would be worse.
const NOT_YET_HONOURED: &[&str] = &[
    "at",
    "catch_up",
    "cwd",
    "disabled",
    "env",
    "jitter",
    "model",
    "on_failure",
    "permission_mode",
    "timeout",
    "tz",
];

/// The frontmatter exactly as written, before validation.
#[derive(Debug, Deserialize)]
struct Frontmatter {
    description: Option<String>,
    cron: Option<String>,
    agent: Option<String>,
    /// Anything this build doesn't act on. Kept rather than dropped so it can
    /// be reported: a key that silently does nothing is the worst outcome.
    #[serde(flatten)]
    extra: BTreeMap<String, serde_yaml_ng::Value>,
}

#[derive(Debug, Clone)]
pub struct TaskDefinition {
    pub description: String,
    pub schedule: Schedule,
    pub agent: Option<String>,
    pub prompt: String,
    /// Non-fatal complaints about the definition. A warned Task still runs.
    pub warnings: Vec<String>,
}

impl TaskDefinition {
    pub fn parse(source: &str) -> Result<Self, TaskError> {
        let (frontmatter, body) = split_frontmatter(source)?;

        let parsed: Frontmatter = serde_yaml_ng::from_str(frontmatter)
            .map_err(|error| TaskError::InvalidFrontmatter(error.to_string()))?;

        let cron = parsed
            .cron
            .ok_or_else(|| TaskError::InvalidFrontmatter("missing `cron`".to_string()))?;
        let description = parsed
            .description
            .ok_or_else(|| TaskError::InvalidFrontmatter("missing `description`".to_string()))?;

        let warnings = parsed
            .extra
            .keys()
            .map(|key| {
                if NOT_YET_HONOURED.contains(&key.as_str()) {
                    format!(
                        "`{key}` is part of the task schema but this build does not act on it yet"
                    )
                } else {
                    format!("unknown frontmatter key: {key}")
                }
            })
            .collect();

        Ok(Self {
            description,
            schedule: Schedule::parse(&cron)?,
            agent: parsed.agent,
            prompt: body.trim().to_string(),
            warnings,
        })
    }

    /// The `description` alone, salvaged from a file that failed to parse, so
    /// a Broken Task can still be listed by the name its author gave it.
    pub fn peek_description(source: &str) -> Option<String> {
        #[derive(Deserialize)]
        struct OnlyDescription {
            description: Option<String>,
        }

        let (frontmatter, _) = split_frontmatter(source).ok()?;
        let parsed: OnlyDescription = serde_yaml_ng::from_str(frontmatter).ok()?;
        parsed.description
    }
}

/// Splits a `---` fenced frontmatter block from the body that follows it.
fn split_frontmatter(source: &str) -> Result<(&str, &str), TaskError> {
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    let after_open = source
        .strip_prefix(FENCE)
        .and_then(|rest| {
            rest.strip_prefix('\n')
                .or_else(|| rest.strip_prefix("\r\n"))
        })
        .ok_or(TaskError::MissingFrontmatter)?;

    for (index, line) in after_open.match_indices(FENCE) {
        let starts_line = index == 0 || after_open[..index].ends_with('\n');
        let rest = &after_open[index + line.len()..];
        let ends_line = rest.is_empty() || rest.starts_with('\n') || rest.starts_with("\r\n");

        if starts_line && ends_line {
            return Ok((&after_open[..index], rest));
        }
    }

    Err(TaskError::UnterminatedFrontmatter)
}
