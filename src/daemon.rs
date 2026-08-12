//! The Daemon: scheduler and runner in one process.
//!
//! Time comes only from the injected [`Clock`], so the whole schedule can be
//! driven deterministically. Every due Tick becomes exactly one Run, and a
//! Run never blocks the scheduler — it is started, then awaited elsewhere.

use crate::clock::Clock;
use crate::config::Config;
use crate::discovery::{self, TaskHealth};
use crate::run::{self, RunRecord, RunStatus, Trigger};
use crate::runner;
use crate::state::State;
use crate::task::TaskDefinition;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

/// A Ready Task plus the instant it next comes due. Broken Tasks never get
/// one of these, which is how "Broken never fires" is enforced.
struct Scheduled {
    id: String,
    project_dir: PathBuf,
    definition: TaskDefinition,
    agent: String,
    next_fire_at: Option<DateTime<Utc>>,
}

pub struct Daemon {
    config: Config,
    clock: Arc<dyn Clock>,
    zone: Tz,
    state_dir: PathBuf,
    state: State,
    scheduled: Vec<Scheduled>,
    /// Broken Tasks seen by the last scan. Reported, never scheduled.
    broken_count: usize,
    /// Runs started and not yet finished.
    running: Vec<JoinHandle<()>>,
}

impl Daemon {
    /// A Daemon evaluating Ticks in the host's local zone.
    pub fn new(config: Config, clock: Arc<dyn Clock>) -> Result<Self> {
        let zone = crate::zone::host();
        Self::with_zone(config, clock, zone)
    }

    /// A Daemon evaluating Ticks in an explicit zone.
    pub fn with_zone(config: Config, clock: Arc<dyn Clock>, zone: Tz) -> Result<Self> {
        // One state root serves the whole machine (ADR-0001); failing to
        // resolve it is fatal, never a silent fallback to somewhere else.
        let state_dir = config.state_dir()?;
        Ok(Self {
            config,
            clock,
            zone,
            state_dir,
            state: State::default(),
            scheduled: Vec::new(),
            broken_count: 0,
            running: Vec::new(),
        })
    }

    pub fn state_dir(&self) -> &PathBuf {
        &self.state_dir
    }

    /// How many Tasks are currently scheduled.
    pub fn task_count(&self) -> usize {
        self.scheduled.len()
    }

    /// How many Runs are in flight.
    pub fn running_count(&self) -> usize {
        self.running.len()
    }

    /// How many Tasks the last scan found Broken.
    pub fn broken_count(&self) -> usize {
        self.broken_count
    }

    /// Rescans every Project and recomputes when each Task next fires.
    pub async fn reload(&mut self) -> Result<()> {
        let now = self.clock.now();
        self.state = State::load(&self.state_dir)?;

        let mut scheduled = Vec::new();
        let mut broken = 0;

        for task in discovery::scan_all(&self.config) {
            self.state
                .upsert(&task.id, &task.path.display().to_string());

            for warning in task.warnings() {
                tracing::warn!(task = %task.id, "{warning}");
            }

            match task.health {
                TaskHealth::Ready { definition, agent } => {
                    let next_fire_at = definition.schedule.next_fire_after(now, &self.zone);
                    scheduled.push(Scheduled {
                        id: task.id,
                        project_dir: task.project_dir,
                        definition: *definition,
                        agent,
                        next_fire_at,
                    });
                }
                TaskHealth::Broken { error, .. } => {
                    // Loud, and still listed: a typo must never look like a
                    // task that simply chose not to run.
                    broken += 1;
                    tracing::error!(task = %task.id, "broken, will not run: {error}");
                }
            }
        }

        scheduled.sort_by(|a, b| a.id.cmp(&b.id));
        self.scheduled = scheduled;
        self.broken_count = broken;

        self.state.save(&self.state_dir)?;
        Ok(())
    }

    /// Starts every Task whose Tick has come due and returns immediately. A
    /// Task that missed several Ticks runs once, not once per Tick — there is
    /// no catch-up.
    pub async fn tick(&mut self) -> Result<()> {
        let now = self.clock.now();

        let due: Vec<(usize, DateTime<Utc>)> = self
            .scheduled
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match entry.next_fire_at {
                Some(next) if next <= now => Some((index, next)),
                _ => None,
            })
            .collect();

        for (index, scheduled_for) in due {
            self.scheduled[index].next_fire_at = self.scheduled[index]
                .definition
                .schedule
                .next_fire_after(now, &self.zone);

            if let Err(error) = self.start_run(index, Some(scheduled_for)) {
                tracing::error!(
                    task = %self.scheduled[index].id,
                    "run failed to start: {error:#}"
                );
            }
        }

        self.running.retain(|handle| !handle.is_finished());
        Ok(())
    }

    /// Awaits every in-flight Run.
    pub async fn wait_for_running(&mut self) {
        for handle in std::mem::take(&mut self.running) {
            let _ = handle.await;
        }
    }

    /// Starts one Run: records it as running, spawns the Agent, and hands the
    /// waiting to a background task so the scheduler stays responsive.
    fn start_run(&mut self, index: usize, scheduled_for: Option<DateTime<Utc>>) -> Result<()> {
        let task = &self.scheduled[index];
        let task_id = task.id.clone();
        let working_dir = task.project_dir.clone();
        let prompt = task.definition.prompt.clone();

        // The Agent was resolved and checked when the Task was scanned; a
        // Task pointing at a missing one is Broken and never reaches here.
        let agent_name = task.agent.clone();
        let template = self
            .config
            .agent(&agent_name)
            .with_context(|| format!("task {task_id} names unknown agent {agent_name:?}"))?
            .cmd
            .clone();
        let command = runner::build_command(&template, &prompt)?;

        let started_at = self.clock.now();
        let (run_id, run_dir) = run::create_run_dir(&self.state_dir, &task_id, started_at)?;

        let mut record = RunRecord {
            run_id,
            task_id: task_id.clone(),
            status: RunStatus::Running,
            exit_code: None,
            trigger: Trigger::Schedule,
            agent: agent_name,
            scheduled_for,
            started_at,
            finished_at: None,
        };
        // Persisted before the Agent starts, so a Run interrupted by a daemon
        // crash still leaves a readable record beside its log.
        record.write_to(&run_dir)?;

        // Header first, then hand the appending file to both streams so the
        // agent's stdout and stderr interleave in their real order.
        let log_path = run_dir.join(run::RUN_LOG);
        std::fs::write(&log_path, run::log_header(&record))
            .with_context(|| format!("writing {}", log_path.display()))?;
        let log = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .with_context(|| format!("opening {}", log_path.display()))?;

        let mut process = tokio::process::Command::new(&command.program);
        process
            .args(&command.args)
            .current_dir(&working_dir)
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .stdin(if command.prompt_on_stdin {
                Stdio::piped()
            } else {
                Stdio::null()
            });

        let mut child = process
            .spawn()
            .with_context(|| format!("spawning agent for {task_id}"))?;

        if let Some(entry) = self.state.task_mut(&task_id) {
            entry.last_run_at = Some(started_at);
            entry.last_scheduled_for = scheduled_for;
        }
        self.state.save(&self.state_dir)?;

        let clock = Arc::clone(&self.clock);
        let piped_prompt = command.prompt_on_stdin.then_some(prompt);

        self.running.push(tokio::spawn(async move {
            if let Some(prompt) = piped_prompt
                && let Some(mut stdin) = child.stdin.take()
                && let Err(error) = stdin.write_all(prompt.as_bytes()).await
            {
                tracing::warn!(task = %record.task_id, "writing prompt to stdin: {error}");
            }

            match child.wait().await {
                Ok(status) => {
                    record.exit_code = status.code();
                    record.status = if status.success() {
                        RunStatus::Succeeded
                    } else {
                        RunStatus::Failed
                    };
                }
                Err(error) => {
                    tracing::error!(task = %record.task_id, "waiting for agent: {error}");
                    record.status = RunStatus::Interrupted;
                }
            }

            record.finished_at = Some(clock.now());
            if let Err(error) = record.write_to(&run_dir) {
                tracing::error!(task = %record.task_id, "recording run: {error:#}");
            }
        }));

        Ok(())
    }
}
