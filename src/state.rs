//! Machine-owned run state: `scheduled-tasks.json`.
//!
//! Disposable by design — delete it and you lose run history, not Tasks.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const STATE_FILE: &str = "scheduled-tasks.json";

/// How many Skips are kept per Task. Enough to explain recent behaviour
/// without letting a frequent Task's history grow without bound.
pub const MAX_SKIPS_PER_TASK: usize = 50;

/// A Tick that was deliberately not run, and why.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum Skip {
    /// The previous Run of this Task was still going.
    #[serde(rename_all = "camelCase")]
    Overlap {
        scheduled_for: DateTime<Utc>,
        recorded_at: DateTime<Utc>,
    },
    /// The Daemon wasn't running. One entry per outage, not per Tick.
    #[serde(rename_all = "camelCase")]
    DaemonDown {
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        count: usize,
        recorded_at: DateTime<Utc>,
        /// Set when the outage was longer than the Daemon counted, so the
        /// figures above are a floor rather than the whole story.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        truncated: bool,
    },
    /// The Task was held when its Tick came due.
    #[serde(rename_all = "camelCase")]
    Paused {
        scheduled_for: DateTime<Utc>,
        recorded_at: DateTime<Utc>,
    },
    /// The Daemon was running but did not reach these Ticks in time — a
    /// suspended laptop, or a scheduler pass that ran long.
    #[serde(rename_all = "camelCase")]
    Missed {
        from: DateTime<Utc>,
        to: DateTime<Utc>,
        count: usize,
        recorded_at: DateTime<Utc>,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    /// Set while every Task is held: the daemon, API, and UI stay up so the
    /// machine can be inspected, but nothing fires.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
    pub scheduled_tasks: Vec<TaskState>,
    /// Keyed by Task id, oldest first.
    pub recorded_skips: BTreeMap<String, Vec<Skip>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskState {
    pub id: String,
    pub file_path: String,
    /// Held at runtime. Distinct from `disabled:` in the file, which travels
    /// with the repository; a Task runs only when neither is set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub paused: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_scheduled_for: Option<DateTime<Utc>>,
    /// The definition, as it was when this Task last started a Run. A Task
    /// whose file no longer matches has changed since anyone exercised it.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_run_digest: Option<String>,
    /// The moment a One-shot has already answered. Editing `at:` to a new
    /// moment makes it a Task with something left to do again.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub completed_for: Option<DateTime<Utc>>,
    /// The most recent Tick this Task answered, whether by running or by
    /// Skipping. Distinct from `last_scheduled_for`, which belongs to the
    /// last actual Run — without it, a Skipped Tick would be counted a
    /// second time as downtime after a restart.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_tick_at: Option<DateTime<Utc>>,
}

impl TaskState {
    pub fn new(id: String, file_path: String) -> Self {
        Self {
            id,
            file_path,
            paused: false,
            last_run_at: None,
            last_scheduled_for: None,
            last_run_digest: None,
            completed_for: None,
            last_tick_at: None,
        }
    }
}

impl State {
    pub fn path_in(state_dir: &Path) -> PathBuf {
        state_dir.join(STATE_FILE)
    }

    /// Reads the state without ever writing.
    ///
    /// A file that cannot be parsed yields an empty state and is left exactly
    /// where it is — setting it aside is the Daemon's job, under its lock.
    pub fn load(state_dir: &Path) -> Result<Self> {
        Self::read(state_dir).map(|(state, _)| state)
    }

    /// Reads the state and, if it is unreadable, sets it aside so a fresh
    /// one can take its place. Only for the holder of the daemon lock.
    pub fn load_and_quarantine(state_dir: &Path, now: DateTime<Utc>) -> Result<Self> {
        let (state, damaged) = Self::read(state_dir)?;
        if let Some(path) = damaged {
            let aside =
                path.with_extension(format!("json.corrupt-{}", now.format("%Y%m%dT%H%M%SZ")));
            tracing::error!(
                "run state at {} could not be read; moving it to {} and starting fresh — \
                 history is lost, tasks are not",
                path.display(),
                aside.display()
            );
            std::fs::rename(&path, &aside)
                .with_context(|| format!("setting aside {}", path.display()))?;
        }
        Ok(state)
    }

    /// The state, plus the path of a file that could not be parsed.
    fn read(state_dir: &Path) -> Result<(Self, Option<PathBuf>)> {
        let path = Self::path_in(state_dir);
        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok((Self::default(), None));
            }
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };

        match serde_json::from_str(&raw) {
            Ok(state) => Ok((state, None)),
            Err(error) => {
                tracing::warn!("run state at {} is unreadable: {error}", path.display());
                Ok((Self::default(), Some(path)))
            }
        }
    }

    /// Writes the state so a reader only ever sees a whole one.
    ///
    /// Written beside the real file and renamed over it: a crash mid-write
    /// leaves either the old content or the new, never a torn mixture.
    pub fn save(&self, state_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(state_dir)
            .with_context(|| format!("creating state dir {}", state_dir.display()))?;
        let path = Self::path_in(state_dir);
        let temporary = path.with_extension("json.tmp");
        let json = serde_json::to_string_pretty(self)?;

        std::fs::write(&temporary, format!("{json}\n"))
            .with_context(|| format!("writing {}", temporary.display()))?;
        std::fs::rename(&temporary, &path).with_context(|| format!("replacing {}", path.display()))
    }

    /// Records a Skip, keeping only the most recent ones.
    pub fn record_skip(&mut self, task_id: &str, skip: Skip) {
        let skips = self.recorded_skips.entry(task_id.to_string()).or_default();
        skips.push(skip);
        let excess = skips.len().saturating_sub(MAX_SKIPS_PER_TASK);
        skips.drain(..excess);
    }

    pub fn last_scheduled_for(&self, id: &str) -> Option<DateTime<Utc>> {
        self.task(id).and_then(|task| task.last_scheduled_for)
    }

    /// Whether this Task is held, individually or by the global pause.
    pub fn is_paused(&self, id: &str) -> bool {
        self.paused || self.task(id).is_some_and(|task| task.paused)
    }

    /// Holds or releases one Task. Returns whether anything changed.
    pub fn set_paused(&mut self, id: &str, paused: bool) -> bool {
        match self.task_mut(id) {
            Some(task) if task.paused != paused => {
                task.paused = paused;
                true
            }
            _ => false,
        }
    }

    /// When this Task last started a Run.
    pub fn last_run_at(&self, id: &str) -> Option<DateTime<Utc>> {
        self.task(id).and_then(|task| task.last_run_at)
    }

    /// The most recent Tick this Task answered, by Run or by Skip.
    pub fn last_tick_at(&self, id: &str) -> Option<DateTime<Utc>> {
        self.task(id).and_then(|task| task.last_tick_at)
    }

    fn task(&self, id: &str) -> Option<&TaskState> {
        self.scheduled_tasks.iter().find(|task| task.id == id)
    }

    /// The moment a One-shot has already answered, if any.
    pub fn completed_for(&self, id: &str) -> Option<DateTime<Utc>> {
        self.task(id).and_then(|task| task.completed_for)
    }

    /// The digest recorded when this Task last started a Run.
    pub fn last_run_digest(&self, id: &str) -> Option<&str> {
        self.task(id)
            .and_then(|task| task.last_run_digest.as_deref())
    }

    /// Forgets Tasks that a successful scan of their own Project did not
    /// find.
    ///
    /// Only Projects that were actually readable count. A directory we could
    /// not open — an unmounted share, a permissions blip — is not evidence
    /// that anything was deleted, and discarding a Task's tick history on
    /// that basis would leave the next outage unaccounted for.
    pub fn prune_missing(
        &mut self,
        present: &std::collections::BTreeSet<&str>,
        reachable_projects: &std::collections::BTreeSet<String>,
    ) -> Vec<String> {
        let mut pruned = Vec::new();
        self.scheduled_tasks.retain(|task| {
            let judged = task
                .id
                .split_once('/')
                .is_some_and(|(project, _)| reachable_projects.contains(project));
            let gone = judged && !present.contains(task.id.as_str());
            if gone {
                pruned.push(task.id.clone());
            }
            !gone
        });
        for id in &pruned {
            self.recorded_skips.remove(id);
        }
        pruned
    }

    /// Marks a Tick as answered, however it was answered.
    pub fn note_tick(&mut self, id: &str, tick: DateTime<Utc>) {
        if let Some(task) = self.task_mut(id) {
            task.last_tick_at = Some(tick);
        }
    }

    pub fn task_mut(&mut self, id: &str) -> Option<&mut TaskState> {
        self.scheduled_tasks.iter_mut().find(|task| task.id == id)
    }

    /// Adds an entry for a newly discovered Task, keeping entries ordered by id.
    pub fn upsert(&mut self, id: &str, file_path: &str) {
        match self.task_mut(id) {
            Some(existing) => existing.file_path = file_path.to_string(),
            None => {
                self.scheduled_tasks
                    .push(TaskState::new(id.to_string(), file_path.to_string()));
                self.scheduled_tasks.sort_by(|a, b| a.id.cmp(&b.id));
            }
        }
    }
}
