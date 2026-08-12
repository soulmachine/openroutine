//! Machine-owned run state: `scheduled-tasks.json`.
//!
//! Disposable by design — delete it and you lose run history, not Tasks.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const STATE_FILE: &str = "scheduled-tasks.json";

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct State {
    pub scheduled_tasks: Vec<TaskState>,
    /// Keyed by Task id. Populated once Skips are recorded.
    pub recorded_skips: BTreeMap<String, Vec<serde_json::Value>>,
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
}

impl TaskState {
    pub fn new(id: String, file_path: String) -> Self {
        Self {
            id,
            file_path,
            enabled: true,
            last_run_at: None,
            last_scheduled_for: None,
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
