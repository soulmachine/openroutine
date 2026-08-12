//! The Task definition: one markdown file, frontmatter plus prompt body.
//!
//! The file is the complete definition — there is no second place to look.
//! Its `name` is the Task's identity, so the definition travels with the file
//! wherever it is moved to.

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

/// Keys that were real and are not any more, with what to write instead.
/// A Task that still carries one would otherwise lose a bound it was relying
/// on and be told only that the key is "unknown", which is the least useful
/// thing we could say about it.
const REPLACED: &[(&str, &str)] = &[
    (
        "timeout",
        "`timeout` bounded a Run's total length and no longer exists; a Run is now \
         ended when it goes quiet, which the daemon sets machine-wide with \
         `idle_timeout` in config.toml",
    ),
    (
        "idle_timeout",
        "`idle_timeout` is set machine-wide in config.toml, not per task; this key \
         does nothing here",
    ),
];

/// The frontmatter exactly as written, before validation.
#[derive(Debug, Deserialize)]
struct Frontmatter {
    /// This Task's identity, unique across the machine.
    name: Option<String>,
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
    /// The Task's name as its author wrote it — prose, punctuation and all.
    /// What a person reads; never what anything is keyed by.
    pub name: String,
    /// The Task's identity, derived from `name` by [`slugify`]. What the
    /// state file, the run directories, the API paths, and every uniqueness
    /// check use.
    pub id: String,
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
    /// Where the Agent starts. Absolute, or relative to the file's directory.
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
            // No schedule at all is still a One-shot: it runs once, as soon as
            // the Daemon takes the definition in.
            (None, None) => Schedule::Once,
        };

        let (name, id) = validate_name(parsed.name)?;

        let description = parsed
            .description
            .ok_or_else(|| TaskError::InvalidFrontmatter("missing `description`".to_string()))?;

        // The body is the prompt. A Task with nothing to say to its Agent is
        // not a Task, and finding that out at 2am is too late.
        let prompt = body.trim().to_string();
        if prompt.is_empty() {
            return Err(TaskError::InvalidFrontmatter(
                "empty prompt: the body after the frontmatter is what the agent is asked to do"
                    .to_string(),
            ));
        }

        let warnings = parsed
            .extra
            .keys()
            .map(|key| {
                if let Some((_, replacement)) =
                    REPLACED.iter().find(|(replaced, _)| replaced == key)
                {
                    (*replacement).to_string()
                } else if NOT_YET_HONOURED.contains(&key.as_str()) {
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
            name,
            id,
            description,
            schedule,
            catch_up: parsed.catch_up,
            disabled: parsed.disabled,
            agent: parsed.agent,
            jitter,
            cwd: parsed.cwd,
            model: parsed.model,
            permission_mode: parsed.permission_mode,
            env: parsed.env,
            prompt,
            warnings,
        })
    }

    /// Where the Agent starts: the directory holding the Task file, or `cwd:`
    /// resolved against it. `Path::join` takes an absolute `cwd` as-is, which
    /// is exactly the intent. One definition, used by the scheduler and by
    /// `--dry-run` alike.
    pub fn working_dir(&self, file_dir: &std::path::Path) -> std::path::PathBuf {
        match &self.cwd {
            Some(cwd) => file_dir.join(cwd),
            None => file_dir.to_path_buf(),
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
        peek(source, |parsed| parsed.description)
    }

    /// The id a file claims, salvaged from frontmatter that may be otherwise
    /// unusable. A Broken Task still needs an identity: it has to be listed,
    /// addressed, and told apart from the next broken file along.
    pub fn peek_id(source: &str) -> Option<String> {
        let name = peek(source, |parsed| parsed.name)?;
        slugify(&name)
            .ok()
            .filter(|id| (ID_MIN..=ID_MAX).contains(&id.chars().count()))
    }
}

/// The two salvageable fields, read from frontmatter that may be otherwise
/// unusable — so neither peek can be broken by the other's absence.
#[derive(Deserialize)]
struct Salvage {
    name: Option<String>,
    description: Option<String>,
}

fn peek(source: &str, pick: impl FnOnce(Salvage) -> Option<String>) -> Option<String> {
    let (frontmatter, _) = split_frontmatter(source).ok()?;
    let parsed: Salvage = serde_yaml_ng::from_str(frontmatter).ok()?;
    pick(parsed).map(|value| value.trim().to_string())
}

/// The shortest id worth having. Two characters is enough to be a word —
/// `qa`, `do` — and one is a typo more often than an intention.
const ID_MIN: usize = 2;
/// Long enough for a sentence's worth of intent, short enough to stay a
/// directory name people can read — and well under the 255-byte ceiling a
/// filesystem puts on one path component.
const ID_MAX: usize = 50;

/// Turns a Task's written name into the id everything else keys by.
///
/// The same derivation Claude Desktop applies when it creates a scheduled
/// task from a name typed in its UI (`app.asar`, `index.chunk-CPdYltki.js`):
/// lowercase, runs of whitespace to a single hyphen, drop anything that is
/// not `[a-z0-9_-]`, then trim hyphens and underscores from both ends.
///
/// Note that punctuation is *deleted* rather than turned into a separator,
/// so `report.v2 daily` becomes `reportv2-daily`. That is faithful to the
/// original, and it is why `list` shows the id beside the name: the
/// conversion is lossy, so the result has to be visible.
///
/// Every character that survives is safe in a path component and in a URL
/// segment, which is what makes the id usable as both.
pub fn slugify(name: &str) -> Result<String, String> {
    // Whitespace first, so the runs that become hyphens are the ones the
    // author actually typed — not gaps left behind by deleted punctuation.
    let mut hyphenated = String::with_capacity(name.len());
    let mut in_whitespace = false;
    for character in name.to_lowercase().chars() {
        if character.is_whitespace() {
            if !in_whitespace {
                hyphenated.push('-');
                in_whitespace = true;
            }
        } else {
            in_whitespace = false;
            hyphenated.push(character);
        }
    }

    let kept: String = hyphenated
        .chars()
        .filter(|character| matches!(character, 'a'..='z' | '0'..='9' | '_' | '-'))
        .collect();
    let id = kept.trim_matches(|character| character == '-' || character == '_');

    if id.is_empty()
        || !id
            .chars()
            .any(|character| character.is_ascii_alphanumeric())
    {
        return Err(format!(
            "{name:?} has no letters or digits to make an id from"
        ));
    }
    Ok(id.to_string())
}

/// The written name, and the id derived from it.
///
/// The name itself is barely constrained — it is a label, and labels are the
/// author's business. What must hold is that it yields a usable id, since the
/// id becomes a directory under `runs/`, a path segment in the API, a key in
/// the state file, and an argument on a command line.
fn validate_name(name: Option<String>) -> Result<(String, String), TaskError> {
    let name = name
        .ok_or_else(|| TaskError::InvalidFrontmatter("missing `name`".to_string()))?
        .trim()
        .to_string();

    if name.is_empty() {
        return Err(TaskError::InvalidFrontmatter(
            "`name` must not be empty".to_string(),
        ));
    }

    let id = slugify(&name).map_err(|reason| {
        TaskError::InvalidFrontmatter(format!("`name` {reason}; give it a word or a number"))
    })?;

    let length = id.chars().count();
    if length < ID_MIN {
        return Err(TaskError::InvalidFrontmatter(format!(
            "`name` {name:?} makes the id {id:?}, which is too short; use at least \
             {ID_MIN} characters"
        )));
    }
    if length > ID_MAX {
        return Err(TaskError::InvalidFrontmatter(format!(
            "`name` {name:?} makes a {length}-character id; keep it to {ID_MAX} or fewer"
        )));
    }
    Ok((name, id))
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
