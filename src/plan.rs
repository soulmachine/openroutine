//! What a Run would do, worked out without doing it.
//!
//! Everything here is derived the same way the scheduler derives it, so a
//! dry run is a description of the real thing rather than a second opinion.

use crate::config::Config;
use crate::discovery::{ScannedTask, TaskHealth};
use crate::runner::{self, AgentParams};
use chrono::{DateTime, TimeZone, Utc};

/// Renders the plan for one Task.
pub fn render<Tz: TimeZone>(
    task: &ScannedTask,
    config: &Config,
    now: DateTime<Utc>,
    zone: &Tz,
) -> Result<String, String>
where
    Tz::Offset: std::fmt::Display,
{
    let (definition, agent) = match &task.health {
        TaskHealth::Ready { definition, agent } => (definition, agent),
        TaskHealth::Broken { error, .. } => return Err(error.clone()),
    };

    let template = config
        .agent(agent)
        .ok_or_else(|| format!("unknown agent {agent:?}"))?;
    let command = runner::login_shell_command(
        runner::build_command(
            &template.cmd,
            &AgentParams {
                prompt: &definition.prompt,
                model: definition.model.as_deref(),
                permission_mode: definition.permission_mode.as_deref(),
            },
        )
        .map_err(|error| error.to_string())?,
        &environment(config, definition),
    );

    let mut out = format!(
        "{id} — {description}\n  agent:    {agent}\n",
        id = task.id,
        description = definition.description,
    );

    // The environment travels through argv as `NAME=VALUE`, and a Task's
    // `env:` is exactly where an API token lives. A plan is the sort of
    // thing people paste into a bug report, so the values never appear.
    let secret_names: Vec<&str> = config
        .env
        .keys()
        .chain(definition.env.keys())
        .map(String::as_str)
        .collect();

    out.push_str("  command:  ");
    out.push_str(&command.program);
    out.push('\n');
    for argument in &command.args {
        out.push_str(&format!(
            "            {:?}\n",
            redact(argument, &secret_names)
        ));
    }

    out.push_str(&format!(
        "  prompt:   {}\n",
        if command.prompt_on_stdin {
            "on stdin"
        } else {
            "in the command above"
        }
    ));

    let working_dir = match &definition.cwd {
        Some(cwd) => task.project_dir.join(cwd),
        None => task.project_dir.clone(),
    };
    out.push_str(&format!("  cwd:      {}\n", working_dir.display()));

    let timeout = definition
        .timeout
        .or_else(|| config.default_timeout().ok())
        .map(|timeout| match timeout {
            crate::task::Timeout::Never => "none".to_string(),
            crate::task::Timeout::After(duration) => humanise(duration),
        })
        .unwrap_or_else(|| "unknown".to_string());
    out.push_str(&format!("  timeout:  {timeout}\n"));

    // Names only. A dry run is the sort of thing people paste into a bug
    // report, and an API token is exactly what a Task's `env:` carries.
    let names: Vec<&str> = config
        .env
        .keys()
        .chain(definition.env.keys())
        .map(String::as_str)
        .collect();
    if !names.is_empty() {
        out.push_str(&format!(
            "  env:      {} (values hidden)\n",
            names.join(", ")
        ));
    }

    out.push_str(&format!(
        "  next:     {}\n",
        next_fire(task, definition, now, zone)
    ));
    Ok(out)
}

/// Replaces the value in a `NAME=VALUE` argument we are about to display.
fn redact(argument: &str, names: &[&str]) -> String {
    match argument.split_once('=') {
        Some((name, _)) if names.contains(&name) => format!("{name}=<hidden>"),
        _ => argument.to_string(),
    }
}

/// A duration as someone would write it, rather than as a machine would.
fn humanise(duration: chrono::Duration) -> String {
    let seconds = duration.num_seconds();
    match () {
        _ if seconds % 86_400 == 0 && seconds > 0 => format!("{}d", seconds / 86_400),
        _ if seconds % 3_600 == 0 && seconds > 0 => format!("{}h", seconds / 3_600),
        _ if seconds % 60 == 0 && seconds > 0 => format!("{}m", seconds / 60),
        _ => format!("{seconds}s"),
    }
}

fn environment(config: &Config, definition: &crate::task::TaskDefinition) -> Vec<(String, String)> {
    config
        .env
        .iter()
        .chain(definition.env.iter())
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn next_fire<Tz: TimeZone>(
    task: &ScannedTask,
    definition: &crate::task::TaskDefinition,
    now: DateTime<Utc>,
    zone: &Tz,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    if definition.schedule.is_manual() {
        return "no schedule — runs only when fired".to_string();
    }

    let Some(tick) = definition.schedule.next_tick_after(now, zone) else {
        return "nothing further — this moment has passed".to_string();
    };

    let fire_at = crate::jitter::fire_at(
        &definition.schedule,
        &task.id,
        definition.jitter,
        tick,
        zone,
    );
    let shown = |instant: DateTime<Utc>| {
        instant
            .with_timezone(zone)
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
    };

    if fire_at == tick {
        shown(fire_at)
    } else {
        format!(
            "{} (tick {}, jittered by {}s)",
            shown(fire_at),
            shown(tick),
            (fire_at - tick).num_seconds()
        )
    }
}
