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
    /// Accepts `2m`, `30s`, or a bare `0`, so it reads naturally either way.
    jitter: Option<serde_yaml_ng::Value>,
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
    /// How far this Task's fire time may be nudged. `None` takes the default.
    pub jitter: Option<chrono::Duration>,
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

        let jitter =
            match parsed.jitter {
                Some(value) => Some(parse_duration_value(&value).map_err(|reason| {
                    TaskError::InvalidFrontmatter(format!("`jitter`: {reason}"))
                })?),
                None => None,
            };

        Ok(Self {
            description,
            schedule: Schedule::parse(&cron)?,
            agent: parsed.agent,
            jitter,
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

/// A duration as frontmatter writes it: `30s`, `5m`, `2h`, `1d`, or a bare
/// number of seconds (so `jitter: 0` reads naturally).
fn parse_duration_value(value: &serde_yaml_ng::Value) -> Result<chrono::Duration, String> {
    if let Some(seconds) = value.as_i64() {
        return non_negative(
            chrono::Duration::try_seconds(seconds)
                .ok_or_else(|| format!("{seconds} seconds is out of range"))?,
        );
    }
    let text = value
        .as_str()
        .ok_or_else(|| "expected a duration like `5m`, or a number of seconds".to_string())?;
    parse_duration(text)
}

pub fn parse_duration(text: &str) -> Result<chrono::Duration, String> {
    let text = text.trim();
    let (digits, unit) = text.split_at(
        text.find(|c: char| !c.is_ascii_digit())
            .unwrap_or(text.len()),
    );

    let amount: i64 = digits
        .parse()
        .map_err(|_| format!("{text:?} is not a duration like `5m`"))?;

    let duration = match unit.trim() {
        "" | "s" => chrono::Duration::seconds(amount),
        "m" => chrono::Duration::minutes(amount),
        "h" => chrono::Duration::hours(amount),
        "d" => chrono::Duration::days(amount),
        other => {
            return Err(format!(
                "unknown duration unit {other:?}; use s, m, h, or d"
            ));
        }
    };
    non_negative(duration)
}

fn non_negative(duration: chrono::Duration) -> Result<chrono::Duration, String> {
    if duration < chrono::Duration::zero() {
        Err("must not be negative".to_string())
    } else {
        Ok(duration)
    }
}
