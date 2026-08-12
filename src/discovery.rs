//! Finding Task files inside a Project.
//!
//! A Task's identity is `<project>/<filename stem>` — the file is the
//! definition, so moving or renaming it makes a different Task.

use crate::config::ProjectConfig;
use crate::task::TaskDefinition;
use anyhow::Result;
use std::path::{Path, PathBuf};

pub const TASK_SUFFIX: &str = ".cron.md";

#[derive(Debug, Clone)]
pub struct DiscoveredTask {
    pub id: String,
    pub path: PathBuf,
    pub project: String,
    pub project_dir: PathBuf,
    pub definition: TaskDefinition,
}

/// Scans one Project for Task files. Unreadable or unparseable files are
/// skipped here; surfacing them as Broken is the validation slice's job.
pub fn discover(project: &ProjectConfig) -> Result<Vec<DiscoveredTask>> {
    let project_name = project.resolved_name();
    let project_dir = project
        .path
        .canonicalize()
        .unwrap_or_else(|_| project.path.clone());

    let mut files = Vec::new();
    collect_task_files(&project_dir, &mut files)?;
    files.sort();

    let mut tasks = Vec::new();
    for path in files {
        let Some(name) = task_name(&path) else {
            continue;
        };
        let Ok(source) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(definition) = TaskDefinition::parse(&source) else {
            continue;
        };
        tasks.push(DiscoveredTask {
            id: format!("{project_name}/{name}"),
            path,
            project: project_name.clone(),
            project_dir: project_dir.clone(),
            definition,
        });
    }
    Ok(tasks)
}

/// The Task's name: the filename with the `.cron.md` suffix removed.
fn task_name(path: &Path) -> Option<String> {
    path.file_name()?
        .to_str()?
        .strip_suffix(TASK_SUFFIX)
        .map(str::to_string)
}

fn collect_task_files(dir: &Path, found: &mut Vec<PathBuf>) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Ok(());
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };

        if file_type.is_dir() {
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            collect_task_files(&path, found)?;
        } else if file_type.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(TASK_SUFFIX))
        {
            found.push(path);
        }
    }
    Ok(())
}
