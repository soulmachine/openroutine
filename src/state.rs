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
    pub scheduled_tasks: Vec<TaskState>,
    /// Keyed by Task id, oldest first.
    pub recorded_skips: BTreeMap<String, Vec<Skip>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskState {
    pub id: String,
    pub file_path: String,
    pub enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_run_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub last_scheduled_for: Option<DateTime<Utc>>,
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
            enabled: true,
            last_run_at: None,
            last_scheduled_for: None,
            last_tick_at: None,
        }
    }
}

impl State {
    pub fn path_in(state_dir: &Path) -> PathBuf {
        state_dir.join(STATE_FILE)
    }

    pub fn load(state_dir: &Path) -> Result<Self> {
        let path = Self::path_in(state_dir);
        match std::fs::read_to_string(&path) {
            Ok(raw) => Ok(serde_json::from_str(&raw)
                .with_context(|| format!("parsing state at {}", path.display()))?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn save(&self, state_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(state_dir)
            .with_context(|| format!("creating state dir {}", state_dir.display()))?;
        let path = Self::path_in(state_dir);
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, format!("{json}\n"))
            .with_context(|| format!("writing {}", path.display()))
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

    /// The most recent Tick this Task answered, by Run or by Skip.
    pub fn last_tick_at(&self, id: &str) -> Option<DateTime<Utc>> {
        self.task(id).and_then(|task| task.last_tick_at)
    }

    fn task(&self, id: &str) -> Option<&TaskState> {
        self.scheduled_tasks.iter().find(|task| task.id == id)
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
