//! The Task definition: one `.cron.md` file, frontmatter plus prompt body.
//!
//! The file is the complete definition — there is no second place to look.

use crate::schedule::{CronSchedule, Schedule};
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
    Schedule(#[from] crate::schedule::ScheduleError),
}

/// Frontmatter keys the v1 schema defines but this build does not act on
/// yet. Warned about specifically: telling someone their `disabled: true` is
/// an "unknown key" would be a lie, and saying nothing would be worse.
const NOT_YET_HONOURED: &[&str] = &["on_failure", "tz"];

/// The frontmatter exactly as written, before validation.
#[derive(Debug, Deserialize)]
struct Frontmatter {
    description: Option<String>,
    cron: Option<String>,
    /// A single moment, RFC 3339. Mutually exclusive with `cron`.
    at: Option<String>,
    #[serde(default)]
    catch_up: bool,
    /// Switched off in the file itself — a reviewable commit rather than
    /// invisible machine state.
    #[serde(default)]
    disabled: bool,
    agent: Option<String>,
    /// Accepts `2m`, `30s`, or a bare `0`, so it reads naturally either way.
    jitter: Option<serde_yaml_ng::Value>,
    /// A duration, or `none` to let the Run take as long as it takes.
    timeout: Option<serde_yaml_ng::Value>,
    cwd: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
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
    /// Whether a Tick missed while the Daemon was away should still run.
    pub catch_up: bool,
    /// Switched off in the file. Distinct from Paused, which is runtime
    /// state; a Task runs only when neither is set.
    pub disabled: bool,
    /// How far this Task's fire time may be nudged. `None` takes the default.
    pub jitter: Option<chrono::Duration>,
    /// How long the Run may take. `None` takes the configured default.
    pub timeout: Option<Timeout>,
    /// Where the Agent starts, relative to the Project root when relative.
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    /// Environment for this Task, layered over the config's own.
    pub env: BTreeMap<String, String>,
    pub prompt: String,
    /// Non-fatal complaints about the definition. A warned Task still runs.
    pub warnings: Vec<String>,
}

impl TaskDefinition {
    pub fn parse(source: &str) -> Result<Self, TaskError> {
        let (frontmatter, body) = split_frontmatter(source)?;

        let parsed: Frontmatter = serde_yaml_ng::from_str(frontmatter)
            .map_err(|error| TaskError::InvalidFrontmatter(error.to_string()))?;

        let schedule = match (&parsed.cron, &parsed.at) {
            (Some(_), Some(_)) => {
                return Err(TaskError::InvalidFrontmatter(
                    "`cron` and `at` are alternatives; a task has one schedule or none".to_string(),
                ));
            }
            (Some(cron), None) => Schedule::Cron(Box::new(CronSchedule::parse(cron)?)),
            (None, Some(at)) => Schedule::At(
                chrono::DateTime::parse_from_rfc3339(at.trim())
                    .map_err(|error| {
                        TaskError::InvalidFrontmatter(format!(
                            "`at`: {at:?} is not an RFC 3339 timestamp ({error})"
                        ))
                    })?
                    .with_timezone(&chrono::Utc),
            ),
            // A Task with no schedule is a Manual one: fireable, never ticked.
            (None, None) => Schedule::Manual,
        };

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

        let timeout =
            match parsed.timeout {
                Some(value) => Some(parse_timeout(&value).map_err(|reason| {
                    TaskError::InvalidFrontmatter(format!("`timeout`: {reason}"))
                })?),
                None => None,
            };

        Ok(Self {
            description,
            schedule,
            catch_up: parsed.catch_up,
            disabled: parsed.disabled,
            agent: parsed.agent,
            jitter,
            timeout,
            cwd: parsed.cwd,
            model: parsed.model,
            permission_mode: parsed.permission_mode,
            env: parsed.env,
            prompt: body.trim().to_string(),
            warnings,
        })
    }

    /// Where the Agent starts: the Project root, or `cwd:` resolved against
    /// it. One definition, used by the scheduler and by `--dry-run` alike.
    pub fn working_dir(&self, project_dir: &std::path::Path) -> std::path::PathBuf {
        match &self.cwd {
            Some(cwd) => project_dir.join(cwd),
            None => project_dir.to_path_buf(),
        }
    }

    /// The environment overrides a Run gets: the config's, then this Task's.
    /// Later entries win, and `env` applies them after the profile has run.
    pub fn environment(&self, config_env: &BTreeMap<String, String>) -> Vec<(String, String)> {
        config_env
            .iter()
            .chain(self.env.iter())
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
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

/// How long a Run may take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timeout {
    After(chrono::Duration),
    /// Explicitly unbounded — the author opted out.
    Never,
}

fn parse_timeout(value: &serde_yaml_ng::Value) -> Result<Timeout, String> {
    if value.as_str().is_some_and(|text| text.trim() == "none") {
        return Ok(Timeout::Never);
    }
    parse_duration_value(value).map(Timeout::After)
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

    // The fallible constructors matter: frontmatter is committer-supplied,
    // and the panicking ones would take the whole Daemon down rather than
    // marking one Task Broken.
    let duration = match unit.trim() {
        "" | "s" => chrono::Duration::try_seconds(amount),
        "m" => chrono::Duration::try_minutes(amount),
        "h" => chrono::Duration::try_hours(amount),
        "d" => chrono::Duration::try_days(amount),
        other => {
            return Err(format!(
                "unknown duration unit {other:?}; use s, m, h, or d"
            ));
        }
    }
    .ok_or_else(|| format!("{text:?} is out of range"))?;

    non_negative(duration)
}

fn non_negative(duration: chrono::Duration) -> Result<chrono::Duration, String> {
    if duration < chrono::Duration::zero() {
        Err("must not be negative".to_string())
    } else {
        Ok(duration)
    }
}
