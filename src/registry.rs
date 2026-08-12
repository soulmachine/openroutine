//! The registered Task files, and whether each one is usable.
//!
//! Nothing is discovered here: the config names every Task file, one path at
//! a time, and this module reads exactly those. A Task's identity is the id
//! derived from the `name` in its own frontmatter, so moving or renaming the
//! file changes nothing. A file that is missing, unreadable, or fails to
//! validate becomes **Broken**: still listed, never fired. Nothing is ever
//! silently skipped.

use crate::config::Config;
use crate::task::TaskDefinition;
use std::path::{Path, PathBuf};

/// Stands in for the digest of a file we could not read, so an unreadable
/// Task is never mistaken for an unchanged one.
const UNREADABLE: &str = "unreadable";

#[derive(Debug, Clone)]
pub struct RegisteredTask {
    pub id: String,
    pub path: PathBuf,
    /// The directory holding the file: what a relative `cwd:` resolves
    /// against, and the working directory when the file names none.
    pub dir: PathBuf,
    pub health: TaskHealth,
    /// A digest of the file as it is right now, for noticing changes.
    pub digest: String,
    /// When the file was last written, as the filesystem reports it. A cheap
    /// pre-filter for a Refresh: unchanged mtime means the file need not be
    /// read at all. Absent when it could not be read.
    pub mtime: Option<std::time::SystemTime>,
    /// Set when this file lost a name collision: an earlier registration
    /// already answers to `id`. Broken like any other unusable Task, but
    /// distinct from one broken on its own merits — the run history and
    /// state filed under that name belong to the winner, not to this file.
    pub duplicate: bool,
}

#[derive(Debug, Clone)]
pub enum TaskHealth {
    /// Parsed, validated, and pointed at an Agent that exists.
    Ready {
        definition: Box<TaskDefinition>,
        agent: String,
    },
    /// Registered but unusable. Carries the reason, verbatim, plus whatever
    /// description survived so the Task can still be described.
    Broken {
        error: String,
        description: Option<String>,
    },
}

impl RegisteredTask {
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
/// Registering a file means trusting whoever can edit it, so a Task arriving
/// or changing is never blocked — but it is never quiet either.
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

/// Reads every registered Task file.
///
/// Config order decides a name collision — the first file to claim a name
/// keeps it — so the list is built in that order and sorted afterwards.
pub fn load_all(config: &Config) -> Vec<RegisteredTask> {
    let mut tasks: Vec<RegisteredTask> = Vec::new();

    for path in &config.tasks {
        let mut task = load_one(path, config);

        if let Some(other) = tasks.iter().find(|earlier| earlier.id == task.id) {
            // Two files answering to one name would make every command
            // ambiguous. The earlier registration keeps the name; the later
            // file stays registered, and says why it cannot run.
            let error = format!(
                "the id {:?} is already taken by {}; give this one a different `name`",
                task.id,
                other.path.display()
            );
            let description = task.description().map(str::to_string);
            task.health = TaskHealth::Broken { error, description };
            task.duplicate = true;
        }
        tasks.push(task);
    }

    tasks.sort_by(|a, b| (&a.id, &a.path).cmp(&(&b.id, &b.path)));
    tasks
}

/// Reads and validates one Task file.
///
/// Digest and health come from a single read: reading twice could digest one
/// edit while assessing another, and the change would never be flagged.
pub fn load_one(path: &Path, config: &Config) -> RegisteredTask {
    // Resolved once, here: everything downstream — state, `cwd`, the paths a
    // user is shown — should name the real file rather than however the
    // config happened to spell it.
    let path = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mtime = std::fs::metadata(&path)
        .and_then(|meta| meta.modified())
        .ok();
    let (digest, id, health) = assess(&path, &dir, config);
    RegisteredTask {
        id,
        path,
        dir,
        health,
        digest,
        mtime,
        // Only `load_all` can see a collision: one file alone never has one.
        duplicate: false,
    }
}

/// The digest, the identity, and the health of one file.
fn assess(path: &Path, dir: &Path, config: &Config) -> (String, String, TaskHealth) {
    // A registered file that isn't there is Broken, not gone: unregistering
    // is something the user does, never something a missing file does.
    let source = match std::fs::read_to_string(path) {
        Ok(source) => source,
        Err(error) => {
            return (
                UNREADABLE.to_string(),
                fallback_id(path),
                TaskHealth::Broken {
                    error: format!("cannot read the task file: {error}"),
                    description: None,
                },
            );
        }
    };
    let digest = crate::digest::of(source.as_bytes());

    // A file too broken to state a usable name still needs an id to be
    // listed and addressed by; the filename is the last resort.
    let id = TaskDefinition::peek_id(&source).unwrap_or_else(|| fallback_id(path));

    let broken = |error: String| {
        (
            digest.clone(),
            id.clone(),
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
    // caught here like any other unusable definition. Absolute is allowed:
    // registering a file is the trust decision, and a Task is entitled to
    // work wherever its author pointed it.
    let working_dir = definition.working_dir(dir);
    if !working_dir.is_dir() {
        return broken(format!(
            "working directory {} does not exist",
            working_dir.display()
        ));
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
        id,
        TaskHealth::Ready {
            definition: Box::new(definition),
            agent,
        },
    )
}

/// The identity of a file that cannot state its own: the filename, with any
/// extension dropped, put through the same derivation a name gets. Enough to
/// list the Task and address it while it is being fixed.
fn fallback_id(path: &Path) -> String {
    let stem = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.split('.').next().unwrap_or(name))
        .unwrap_or_default();

    // A filename can hold characters a name never would, so it goes through
    // the same conversion — and when even that yields nothing usable, the
    // Task still needs to be called something.
    crate::task::slugify(stem)
        .ok()
        .map(|id| id.chars().take(50).collect::<String>())
        .map(|id| id.trim_end_matches(['-', '_']).to_string())
        .filter(|id| id.chars().count() >= 2)
        .unwrap_or_else(|| "unnamed".to_string())
}
