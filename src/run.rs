//! Run records on disk: one directory per Run, holding `run.json` and the
//! merged `output.log`. Everything the tool generates is a file you can read.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const RUNS_DIR: &str = "runs";
pub const RUN_RECORD: &str = "run.json";
pub const RUN_LOG: &str = "output.log";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunStatus {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Interrupted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Trigger {
    Schedule,
    Fire,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub run_id: String,
    pub task_id: String,
    pub status: RunStatus,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub exit_code: Option<i32>,
    pub trigger: Trigger,
    pub agent: String,
    /// The Tick this Run answers. Absent for Runs that were Fired.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub scheduled_for: Option<DateTime<Utc>>,
    pub started_at: DateTime<Utc>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub finished_at: Option<DateTime<Utc>>,
}

impl RunRecord {
    /// Writes the record beside itself and renames, so a crash mid-write
    /// leaves the previous record rather than a torn one.
    pub fn write_to(&self, run_dir: &Path) -> Result<()> {
        let path = run_dir.join(RUN_RECORD);
        let temporary = run_dir.join("run.json.tmp");
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&temporary, format!("{json}\n"))
            .with_context(|| format!("writing {}", temporary.display()))?;
        std::fs::rename(&temporary, &path).with_context(|| format!("replacing {}", path.display()))
    }
}

/// Where a Task's Runs live: `runs/<project>/<task>/`.
pub fn task_runs_dir(state_dir: &Path, task_id: &str) -> PathBuf {
    let (project, name) = task_id.split_once('/').unwrap_or(("_", task_id));
    state_dir.join(RUNS_DIR).join(project).join(name)
}

/// Creates this Run's directory, deriving the id from its start instant and
/// suffixing on the rare same-second collision.
pub fn create_run_dir(
    state_dir: &Path,
    task_id: &str,
    started_at: DateTime<Utc>,
) -> Result<(String, PathBuf)> {
    let base = started_at.format("%Y%m%dT%H%M%SZ").to_string();
    let parent = task_runs_dir(state_dir, task_id);
    std::fs::create_dir_all(&parent).with_context(|| format!("creating {}", parent.display()))?;

    for suffix in 1..1000 {
        let run_id = if suffix == 1 {
            base.clone()
        } else {
            // Padded so `-10` still sorts after `-2`: run directories are
            // ordered by name wherever age matters.
            format!("{base}-{suffix:03}")
        };
        let dir = parent.join(&run_id);
        match std::fs::create_dir(&dir) {
            Ok(()) => return Ok((run_id, dir)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).with_context(|| format!("creating {}", dir.display())),
        }
    }
    anyhow::bail!("could not allocate a run directory for {task_id} at {base}")
}

/// The log's opening header — what ran, when, and why.
pub fn log_header(record: &RunRecord) -> String {
    let scheduled = record
        .scheduled_for
        .map(|when| when.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
        .unwrap_or_else(|| "-".to_string());

    format!(
        "# openroutine run {run_id}\n\
         # task: {task_id}\n\
         # agent: {agent}\n\
         # trigger: {trigger}\n\
         # scheduled: {scheduled}\n\
         # started: {started}\n\
         ---\n",
        run_id = record.run_id,
        task_id = record.task_id,
        agent = record.agent,
        trigger = match record.trigger {
            Trigger::Schedule => "schedule",
            Trigger::Fire => "fire",
        },
        scheduled = scheduled,
        started = record
            .started_at
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
    )
}

/// Copies a Run's output into its log, stopping at `cap` bytes.
///
/// The pipe keeps draining past the cap so a chatty Agent never blocks on a
/// full buffer; the excess is simply discarded, with one line saying so.
pub fn capture_output(mut reader: std::io::PipeReader, log_path: &Path, cap: u64) -> Result<u64> {
    use std::io::{Read, Write};

    let mut log = std::fs::OpenOptions::new()
        .append(true)
        .open(log_path)
        .with_context(|| format!("opening {}", log_path.display()))?;

    let mut buffer = vec![0u8; 64 * 1024];
    let mut written: u64 = 0;
    let mut discarded: u64 = 0;

    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error).context("reading agent output"),
        };

        let room = cap.saturating_sub(written) as usize;
        if room > 0 {
            let take = room.min(read);
            log.write_all(&buffer[..take])
                .context("writing agent output")?;
            written += take as u64;
        }
        discarded += (read - room.min(read)) as u64;
    }

    if discarded > 0 {
        writeln!(
            log,
            "\n--- output truncated at {cap} bytes; {discarded} more discarded ---"
        )
        .context("writing the truncation marker")?;
    }
    log.flush().context("flushing the log")?;
    Ok(written)
}

/// Keeps only the most recent `keep` Runs of a Task.
///
/// A frequent Task would otherwise fill a disk with history nobody reads.
pub fn prune_runs(state_dir: &Path, task_id: &str, keep: usize) -> Result<usize> {
    let dir = task_runs_dir(state_dir, task_id);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };

    // Run ids are sortable start instants, so name order is age order.
    let mut runs: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    runs.sort();

    let mut removed = 0;
    for old in runs.iter().take(runs.len().saturating_sub(keep)) {
        std::fs::remove_dir_all(old).with_context(|| format!("pruning {}", old.display()))?;
        removed += 1;
    }
    Ok(removed)
}

/// Marks Runs that were still `running` when the Daemon disappeared.
///
/// Only ever called while holding the single-daemon lock, so a record left
/// running belongs to a process that is gone.
pub fn close_abandoned_runs(state_dir: &Path, finished_at: DateTime<Utc>) -> usize {
    let mut closed = 0;
    let root = state_dir.join(RUNS_DIR);

    for record in walk_run_records(&root) {
        let raw = match std::fs::read_to_string(&record) {
            Ok(raw) => raw,
            Err(error) => {
                tracing::warn!(run = %record.display(), "cannot read run record: {error}");
                continue;
            }
        };
        let mut parsed = match serde_json::from_str::<RunRecord>(&raw) {
            Ok(parsed) => parsed,
            Err(error) => {
                tracing::warn!(run = %record.display(), "run record is unreadable: {error}");
                continue;
            }
        };
        if parsed.status != RunStatus::Running {
            continue;
        }

        parsed.status = RunStatus::Interrupted;
        parsed.finished_at = Some(finished_at);
        if let Some(dir) = record.parent() {
            match parsed.write_to(dir) {
                Ok(()) => {
                    tracing::warn!(task = %parsed.task_id, run = %parsed.run_id, "run was interrupted");
                    closed += 1;
                }
                Err(error) => tracing::warn!(
                    run = %record.display(),
                    "cannot record that this run was interrupted: {error:#}"
                ),
            }
        }
    }
    closed
}

/// Every `run.json` under `runs/<project>/<task>/<run>/`.
fn walk_run_records(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(projects) = std::fs::read_dir(root) else {
        return found;
    };
    for project in projects.flatten() {
        let Ok(tasks) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for task in tasks.flatten() {
            let Ok(runs) = std::fs::read_dir(task.path()) else {
                continue;
            };
            for run in runs.flatten() {
                let record = run.path().join(RUN_RECORD);
                if record.is_file() {
                    found.push(record);
                }
            }
        }
    }
    found
}
