//! Finding Task files inside a Project, and deciding whether each one is
//! usable.
//!
//! A Task's identity is `<project>/<filename stem>` — the file is the
//! definition, so moving or renaming it makes a different Task. A file that
//! fails to parse or validate becomes **Broken**: still listed, never fired.
//! Nothing here is ever silently skipped.

use crate::config::{Config, ProjectConfig};
use crate::task::TaskDefinition;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub const TASK_SUFFIX: &str = ".cron.md";
/// Stands in for the digest of a file we could not read, so an unreadable
/// Task is never mistaken for an unchanged one.
const UNREADABLE: &str = "unreadable";

#[derive(Debug, Clone)]
pub struct ScannedTask {
    pub id: String,
    pub path: PathBuf,
    pub project: String,
    pub project_dir: PathBuf,
    pub health: TaskHealth,
    /// A digest of the file as it is right now, for noticing changes.
    pub digest: String,
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

    /// Whether the file itself says not to run this.
    pub fn is_disabled(&self) -> bool {
        self.definition()
            .is_some_and(|definition| definition.disabled)
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

    /// How this Task stands relative to the last time it ran.
    pub fn novelty(&self, last_run_digest: Option<&str>) -> Novelty {
        match last_run_digest {
            None => Novelty::NeverRun,
            Some(seen) if seen != self.digest => Novelty::ChangedSinceLastRun,
            Some(_) => Novelty::Familiar,
        }
    }

    pub fn warnings(&self) -> &[String] {
        self.definition()
            .map(|definition| definition.warnings.as_slice())
            .unwrap_or_default()
    }
}

/// Whether a Task is one anybody has exercised in its current form.
///
/// Registering a Project means trusting whoever can commit to it, so a Task
/// arriving or changing is never blocked — but it is never quiet either.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Novelty {
    /// No Run has ever started for this Task.
    NeverRun,
    /// The definition differs from the one that last ran.
    ChangedSinceLastRun,
    /// Last run in exactly this form.
    Familiar,
}

impl Novelty {
    pub fn note(self) -> Option<&'static str> {
        match self {
            Novelty::NeverRun => Some("new: this task has never run"),
            Novelty::ChangedSinceLastRun => {
                Some("changed: the definition differs from the one that last ran")
            }
            Novelty::Familiar => None,
        }
    }
}

/// Everything one pass over the Projects found.
#[derive(Debug, Default)]
pub struct Scan {
    pub tasks: Vec<ScannedTask>,
    /// Projects whose directory was readable this pass. A Task missing from
    /// one of these is genuinely gone; a Task under any other Project is
    /// merely out of reach, which is not the same thing.
    pub reachable_projects: BTreeSet<String>,
}

/// Scans every registered Project.
pub fn scan_all(config: &Config) -> Scan {
    let mut scan = Scan::default();
    for project in &config.projects {
        let name = project.resolved_name();
        // Listable, not merely present: a directory can be stat-able while
        // its contents are unreadable, and finding no Tasks there is not the
        // same as there being none.
        if std::fs::read_dir(&project.path).is_ok() {
            scan.reachable_projects.insert(name);
        }
        scan.tasks.extend(scan_project(project, config));
    }
    scan.tasks.sort_by(|a, b| a.id.cmp(&b.id));
    scan
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
            let (digest, health) = assess(&path, &project_dir, config);
            Some(ScannedTask {
                id: format!("{project_name}/{name}"),
                health,
                digest,
                path,
                project: project_name.clone(),
                project_dir: project_dir.clone(),
            })
        })
        .collect()
}

/// Reads and validates one Task file, returning its digest and its health
/// from a single read — reading twice could digest one edit while assessing
/// another, and the change would never be flagged.
fn assess(path: &Path, project_dir: &Path, config: &Config) -> (String, TaskHealth) {
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            return (
                UNREADABLE.to_string(),
                TaskHealth::Broken {
                    error: format!("cannot read the task file: {error}"),
                    description: None,
                },
            );
        }
    };
    let digest = crate::digest::of(source.as_bytes());

    let broken = |error: String| {
        (
            digest.clone(),
            TaskHealth::Broken {
                error,
                description: TaskDefinition::peek_description(&source),
            },
        )
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

        // An Agent whose program cannot be found will fail at fire time, in
        // the dark. Warned rather than Broken: a login profile may put it on
        // PATH conditionally, and refusing to schedule would be too strong.
        if let Ok(command) = crate::runner::build_command(&template.cmd, &Default::default())
            && let reach = crate::runner::program_reach(&command.program)
            && reach != crate::runner::ProgramReach::Findable
        {
            // An absolute path either exists or it does not; a bare name is
            // looked up on PATH, and a bare name your terminal can resolve
            // but a service manager cannot is a third thing again. Saying
            // the wrong one sends people the wrong way when they go to fix
            // it — and the third is the one that costs an unattended night.
            let program = &command.program;
            let named = *program == agent;
            let interactive = reach == crate::runner::ProgramReach::InteractiveOnly;
            definition
                .warnings
                .push(match (program.contains('/'), interactive, named) {
                    (true, _, _) => format!(
                        "agent {agent:?} runs {program:?}, which is not there or is not executable"
                    ),
                    (false, true, true) => format!(
                        "{agent:?} is on your PATH here but not under a service manager, so \
                         scheduled Runs will fail; move its PATH export into your login profile"
                    ),
                    (false, true, false) => format!(
                        "agent {agent:?} runs {program:?}, which is on your PATH here but not \
                         under a service manager; move its PATH export into your login profile"
                    ),
                    (false, false, true) => format!(
                        "{agent:?} is not on your login shell's PATH, so its Runs will fail"
                    ),
                    (false, false, false) => format!(
                        "agent {agent:?} runs {program:?}, which is not on your login shell's PATH"
                    ),
                });
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

    (
        digest,
        TaskHealth::Ready {
            definition: Box::new(definition),
            agent,
        },
    )
}

/// The Task's name: the filename with the `.cron.md` suffix removed.
fn task_name(path: &Path) -> Option<String> {
    path.file_name()?
        .to_str()?
        .strip_suffix(TASK_SUFFIX)
        .map(str::to_string)
}

/// Walks a Project for Task files.
///
/// Uses the same rules a developer already expects from their tools: what
/// `.gitignore` excludes is not a Task, `.git` is never searched, and a
/// symlink is not followed — a link is not a reason to schedule a file the
/// Project does not contain.
fn collect_task_files(dir: &Path, found: &mut Vec<PathBuf>) {
    let walker = ignore::WalkBuilder::new(dir)
        .follow_links(false)
        .hidden(false)
        .git_ignore(true)
        .git_global(false)
        .git_exclude(true)
        .require_git(false)
        .parents(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .build();

    for entry in walker {
        match entry {
            Ok(entry) => {
                let is_file = entry.file_type().is_some_and(|kind| kind.is_file());
                if is_file
                    && entry
                        .file_name()
                        .to_str()
                        .is_some_and(|name| name.ends_with(TASK_SUFFIX))
                {
                    found.push(entry.into_path());
                }
            }
            // Tasks under something unreadable would otherwise vanish from
            // every listing without explanation.
            Err(error) => tracing::warn!(dir = %dir.display(), "cannot scan for tasks: {error}"),
        }
    }
}
