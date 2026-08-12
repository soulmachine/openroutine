//! The `list` report: what runs here, what's wrong, and when it next fires.
//!
//! Rendered from the registered Task files alone, so it answers with the
//! Daemon stopped — and answers for disk, which a running Daemon may not yet
//! have Reloaded.

use crate::registry::{RegisteredTask, TaskHealth};
use chrono::{DateTime, SecondsFormat, TimeZone, Utc};

const NOT_APPLICABLE: &str = "-";
const COLUMNS: usize = 6;
const HEADINGS: [&str; COLUMNS] = [
    "STATUS",
    "ID",
    "DESCRIPTION",
    "SCHEDULE",
    "AGENT",
    "NEXT TICK",
];

/// One rendered row, before column widths are known.
struct Row {
    cells: [String; COLUMNS],
    notes: Vec<String>,
}

/// Renders every Task, its health, and its next Tick as seen from `now`.
pub fn render<Tz: TimeZone>(
    tasks: &[RegisteredTask],
    state: &crate::state::State,
    now: DateTime<Utc>,
    zone: &Tz,
) -> String
where
    Tz::Offset: std::fmt::Display,
{
    if tasks.is_empty() {
        return "No tasks found.\n".to_string();
    }

    let rows: Vec<Row> = tasks
        .iter()
        .map(|task| row_for(task, state, now, zone))
        .collect();

    let mut widths = HEADINGS.map(str::len);
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(&row.cells) {
            *width = (*width).max(cell.chars().count());
        }
    }

    let mut out = String::new();
    push_cells(&mut out, &HEADINGS.map(str::to_string), &widths);
    for row in &rows {
        push_cells(&mut out, &row.cells, &widths);
        for note in &row.notes {
            out.push_str("  ");
            out.push_str(note);
            out.push('\n');
        }
    }

    out.push('\n');
    out.push_str(&summarize(tasks));
    out
}

fn row_for<Tz: TimeZone>(
    task: &RegisteredTask,
    state: &crate::state::State,
    now: DateTime<Utc>,
    zone: &Tz,
) -> Row
where
    Tz::Offset: std::fmt::Display,
{
    let mut notes: Vec<String> = task
        .novelty(state.last_run_digest(&task.id))
        .note()
        .map(str::to_string)
        .into_iter()
        .chain(
            task.warnings()
                .iter()
                .map(|warning| format!("warning: {warning}")),
        )
        .collect();

    // Descriptions come from whoever can edit the file; a newline would
    // otherwise break the row apart.
    let description = crate::text::one_line(task.description().unwrap_or(NOT_APPLICABLE));

    let cells = match &task.health {
        TaskHealth::Ready { definition, agent } if definition.disabled => [
            "disabled".to_string(),
            task.id.clone(),
            description,
            definition.schedule.expression(),
            agent.clone(),
            NOT_APPLICABLE.to_string(),
        ],
        TaskHealth::Ready { definition, agent } if state.is_paused(&task.id) => [
            "paused".to_string(),
            task.id.clone(),
            description,
            definition.schedule.expression(),
            agent.clone(),
            NOT_APPLICABLE.to_string(),
        ],
        TaskHealth::Ready { definition, agent } => {
            let next_fire = definition
                .schedule
                .next_tick_after(now, zone)
                .map(|when| {
                    when.with_timezone(zone)
                        .to_rfc3339_opts(SecondsFormat::Secs, false)
                })
                .unwrap_or_else(|| NOT_APPLICABLE.to_string());

            [
                "ready".to_string(),
                task.id.clone(),
                description,
                definition.schedule.expression().to_string(),
                agent.clone(),
                next_fire,
            ]
        }
        TaskHealth::Broken { error, .. } => {
            notes.push(format!("error: {error}"));
            [
                "broken".to_string(),
                task.id.clone(),
                description,
                NOT_APPLICABLE.to_string(),
                NOT_APPLICABLE.to_string(),
                NOT_APPLICABLE.to_string(),
            ]
        }
    };

    Row { cells, notes }
}

fn push_cells(out: &mut String, cells: &[String; COLUMNS], widths: &[usize; COLUMNS]) {
    let last = cells.len() - 1;
    for (index, (cell, width)) in cells.iter().zip(widths).enumerate() {
        if index == last {
            out.push_str(cell);
        } else {
            out.push_str(&format!("{cell:<width$}  "));
        }
    }
    out.push('\n');
}

fn summarize(tasks: &[RegisteredTask]) -> String {
    let broken = tasks.iter().filter(|task| task.is_broken()).count();
    let disabled = tasks.iter().filter(|task| task.is_disabled()).count();
    let warned = tasks
        .iter()
        .filter(|task| !task.warnings().is_empty())
        .count();

    let mut parts = vec![format!("{} ready", tasks.len() - broken - disabled)];
    if disabled > 0 {
        parts.push(format!("{disabled} disabled"));
    }
    if broken > 0 {
        parts.push(format!("{broken} broken"));
    }
    if warned > 0 {
        parts.push(format!("{warned} with warnings"));
    }

    format!(
        "{} task{}: {}\n",
        tasks.len(),
        if tasks.len() == 1 { "" } else { "s" },
        parts.join(", ")
    )
}
