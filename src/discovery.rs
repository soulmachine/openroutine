//! Finding Task files inside a Project, and deciding whether each one is
//! usable.
//!
//! A Task's identity is `<project>/<filename stem>` — the file is the
//! definition, so moving or renaming it makes a different Task. A file that
//! fails to parse or validate becomes **Broken**: still listed, never fired.
//! Nothing here is ever silently skipped.

use crate::config::{Config, ProjectConfig};
use crate::task::TaskDefinition;
use std::path::{Path, PathBuf};

pub const TASK_SUFFIX: &str = ".cron.md";

#[derive(Debug, Clone)]
pub struct ScannedTask {
    pub id: String,
    pub path: PathBuf,
    pub project: String,
    pub project_dir: PathBuf,
    pub health: TaskHealth,
}

#[derive(Debug, Clone)]
pub enum TaskHealth {
    /// Parsed, validated, and pointed at an Agent that exists.
    Ready {
        definition: Box<TaskDefinition>,
        agent: String,
    },
    /// The file exists but can't be used. Carries the reason, verbatim, plus
    /// whatever description survived so the Task can still be named.
    Broken {
        error: String,
        description: Option<String>,
    },
}

impl ScannedTask {
    pub fn is_broken(&self) -> bool {
        matches!(self.health, TaskHealth::Broken { .. })
    }

    pub fn definition(&self) -> Option<&TaskDefinition> {
        match &self.health {
            TaskHealth::Ready { definition, .. } => Some(definition),
            TaskHealth::Broken { .. } => None,
        }
    }

    /// The Task's description, whether or not it is usable.
    pub fn description(&self) -> Option<&str> {
        match &self.health {
            TaskHealth::Ready { definition, .. } => Some(&definition.description),
            TaskHealth::Broken { description, .. } => description.as_deref(),
        }
    }

    pub fn warnings(&self) -> &[String] {
        self.definition()
            .map(|definition| definition.warnings.as_slice())
            .unwrap_or_default()
    }
}

/// Scans every registered Project.
pub fn scan_all(config: &Config) -> Vec<ScannedTask> {
    let mut tasks: Vec<ScannedTask> = config
        .projects
        .iter()
        .flat_map(|project| scan_project(project, config))
        .collect();
    tasks.sort_by(|a, b| a.id.cmp(&b.id));
    tasks
}

/// Scans one Project, reporting every Task file it contains.
pub fn scan_project(project: &ProjectConfig, config: &Config) -> Vec<ScannedTask> {
    let project_name = project.resolved_name();
    let project_dir = project
        .path
        .canonicalize()
        .unwrap_or_else(|_| project.path.clone());

    let mut files = Vec::new();
    collect_task_files(&project_dir, &mut files);
    files.sort();

    files
        .into_iter()
        .filter_map(|path| {
            let name = task_name(&path)?;
            Some(ScannedTask {
                id: format!("{project_name}/{name}"),
                health: assess(&path, &project_dir, config),
                path,
                project: project_name.clone(),
                project_dir: project_dir.clone(),
            })
        })
        .collect()
}

/// Reads and validates one Task file.
fn assess(path: &Path, project_dir: &Path, config: &Config) -> TaskHealth {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            return TaskHealth::Broken {
                error: format!("cannot read the task file: {error}"),
                description: None,
            };
        }
    };

    let broken = |error: String| TaskHealth::Broken {
        error,
        description: TaskDefinition::peek_description(&source),
    };

    let mut definition = match TaskDefinition::parse(&source) {
        Ok(definition) => definition,
        Err(error) => return broken(error.to_string()),
    };

    // A working directory that isn't there would fail at fire time, so it is
    // caught here like any other unusable definition.
    if let Some(cwd) = &definition.cwd {
        let candidate = Path::new(cwd);
        // "Relative to the Project root" is the whole contract; an absolute
        // path or a `..` climb would put the Agent somewhere the Project does
        // not own.
        if candidate.is_absolute()
            || candidate
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return broken(format!(
                "working directory {cwd:?} must be relative to the project and stay inside it"
            ));
        }
        if !project_dir.join(candidate).is_dir() {
            return broken(format!(
                "working directory {cwd:?} does not exist under the project"
            ));
        }
    }

    let agent = match config.resolve_agent(definition.agent.as_deref()) {
        Ok(agent) => agent,
        Err(error) => return broken(error),
    };

    // A template that can't be turned into a command would fail at fire time,
    // in the dark. Catch it while someone is looking.
    if let Some(template) = config.agent(&agent) {
        if let Err(error) = crate::runner::validate_template(&template.cmd) {
            return broken(error);
        }

        // A parameter the template has nowhere to put is not fatal — the Run
        // still works — but it silently does nothing, so say so.
        let placeholders = crate::runner::template_placeholders(&template.cmd);
        for (field, value) in [
            (crate::runner::MODEL, &definition.model),
            (crate::runner::PERMISSION_MODE, &definition.permission_mode),
        ] {
            if value.is_some() && !placeholders.contains(field) {
                definition.warnings.push(format!(
                    "`{field}` is set but agent {agent:?} has no {{{field}}} in its command, so it is ignored"
                ));
            }
        }
    }

    TaskHealth::Ready {
        definition: Box::new(definition),
        agent,
    }
}

/// The Task's name: the filename with the `.cron.md` suffix removed.
fn task_name(path: &Path) -> Option<String> {
    path.file_name()?
        .to_str()?
        .strip_suffix(TASK_SUFFIX)
        .map(str::to_string)
}

fn collect_task_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            // Tasks under an unreadable directory would otherwise vanish from
            // every listing without explanation.
            tracing::warn!(dir = %dir.display(), "cannot scan for tasks: {error}");
            return;
        }
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
            collect_task_files(&path, found);
        } else if file_type.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with(TASK_SUFFIX))
        {
            found.push(path);
        }
    }
}
