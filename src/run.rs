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
    pub fn write_to(&self, run_dir: &Path) -> Result<()> {
        let path = run_dir.join(RUN_RECORD);
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, format!("{json}\n"))
            .with_context(|| format!("writing {}", path.display()))
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
            format!("{base}-{suffix}")
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
