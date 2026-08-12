//! `CRONTAB.md`: the glanceable answer to "what runs here, and when?"
//!
//! Derived from the Task definitions alone — never from run state — so the
//! file changes only when a definition changes and is safe to commit.

use crate::config::Config;
use crate::discovery::ScannedTask;
use std::collections::BTreeMap;
use std::path::Path;

pub const CRONTAB_FILE: &str = "CRONTAB.md";
const HEADER: &str = "# CRONTAB.md\n\n\
     <!-- Written by openroutine from the *.cron.md files here. Safe to commit. -->\n\n\
     | Task | Description | Schedule | Agent |\n\
     | :-- | :-- | :-- | :-- |\n";

/// Regenerates every Project's `CRONTAB.md`, skipping those that opted out.
pub fn write_all(tasks: &[ScannedTask], config: &Config) {
    let mut by_project: BTreeMap<&str, Vec<&ScannedTask>> = BTreeMap::new();
    for task in tasks {
        by_project.entry(&task.project).or_default().push(task);
    }

    for project in &config.projects {
        if !project.crontab_md.unwrap_or(true) {
            continue;
        }
        let name = project.resolved_name();
        let dir = match project.path.canonicalize() {
            Ok(dir) => dir,
            Err(error) => {
                tracing::warn!(project = %name, "cannot reach {}: {error}", project.path.display());
                continue;
            }
        };
        let tasks = by_project.remove(name.as_str()).unwrap_or_default();
        if let Err(error) = write_one(&dir, &tasks) {
            tracing::warn!(project = %name, "{error}");
        }
    }
}

fn write_one(dir: &Path, tasks: &[&ScannedTask]) -> Result<(), String> {
    let path = dir.join(CRONTAB_FILE);

    // Writing through a symlink would put our output somewhere nobody
    // pointed us at.
    if path
        .symlink_metadata()
        .is_ok_and(|meta| meta.file_type().is_symlink())
    {
        return Err(format!(
            "{} is a symlink; refusing to write through it",
            path.display()
        ));
    }

    let rendered = render(tasks);
    // Only touch the file when the content actually differs, so a repo does
    // not show a modification for a scan that changed nothing.
    if std::fs::read_to_string(&path).is_ok_and(|existing| existing == rendered) {
        return Ok(());
    }
    // O_NOFOLLOW closes the gap between the check above and the write: a
    // link appearing in between must fail, not redirect our output.
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&path)
        .map_err(|error| format!("writing {}: {error}", path.display()))?;
    std::io::Write::write_all(&mut file, rendered.as_bytes())
        .map_err(|error| format!("writing {}: {error}", path.display()))
}

fn render(tasks: &[&ScannedTask]) -> String {
    let mut out = String::from(HEADER);
    for task in tasks {
        let name = task.id.split_once('/').map_or(task.id.as_str(), |(_, n)| n);
        let (schedule, agent) = match &task.health {
            crate::discovery::TaskHealth::Ready { definition, agent } => (
                format!("`{}`", definition.schedule.expression()),
                agent.clone(),
            ),
            crate::discovery::TaskHealth::Broken { .. } => {
                ("—".to_string(), "(broken)".to_string())
            }
        };
        out.push_str(&format!(
            "| {name} | {description} | {schedule} | {agent} |\n",
            name = crate::text::table_cell(name),
            description = crate::text::table_cell(task.description().unwrap_or("—")),
            schedule = crate::text::table_cell(&schedule),
            agent = crate::text::table_cell(&agent),
        ));
    }
    if tasks.is_empty() {
        out.push_str("| _none_ | | | |\n");
    }
    out
}
