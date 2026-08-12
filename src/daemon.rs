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
use crate::state::{Skip, State};
use crate::task::TaskDefinition;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::AsyncWriteExt;
use tokio::task::JoinHandle;

/// How many missed Ticks are counted before giving up on an exact figure.
/// A minutely Task down for a month is ~43k; the bound keeps a pathological
/// outage from stalling startup.
const MAX_COUNTED_MISSES: usize = 50_000;

/// A Ready Task plus when it next comes due. Broken Tasks never get one of
/// these, which is how "Broken never fires" is enforced.
struct Scheduled {
    id: String,
    project_dir: PathBuf,
    definition: TaskDefinition,
    agent: String,
    /// The Tick itself — what the schedule promised, and what a Run records.
    next_tick: Option<DateTime<Utc>>,
    /// When it actually fires: the Tick plus this Task's jitter offset.
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
    /// Runs started and not yet finished, by Task id — a Task appearing here
    /// is why its next Tick becomes an overlap Skip.
    running: HashMap<String, JoinHandle<()>>,
    /// Whether this Daemon has scanned yet. Downtime is a startup question:
    /// only the first scan can tell missed Ticks from ordinary ones.
    has_scanned: bool,
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
            running: HashMap::new(),
            has_scanned: false,
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
                    if !self.has_scanned {
                        self.record_downtime(&task.id, &definition, now);
                    }
                    let next_tick = self.first_tick(&task.id, &definition, now);
                    scheduled.push(Scheduled {
                        next_tick,
                        next_fire_at: next_tick
                            .map(|tick| self.fire_at(&task.id, &definition, tick)),
                        id: task.id,
                        project_dir: task.project_dir,
                        definition: *definition,
                        agent,
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
        self.has_scanned = true;

        self.state.save(&self.state_dir)?;
        Ok(())
    }

    /// The Tick a freshly loaded Task is waiting for.
    ///
    /// Two traps here. A Tick due at exactly this instant must not be stepped
    /// over, and a Tick whose jittered fire time hasn't arrived yet is still
    /// pending even though the Tick itself is in the past — reloading (which
    /// hot reload makes frequent) must not drop it.
    fn first_tick(
        &self,
        id: &str,
        definition: &TaskDefinition,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        let lookback = definition
            .jitter
            .unwrap_or(crate::jitter::DEFAULT_WINDOW)
            .max(crate::jitter::DEFAULT_WINDOW);
        let answered = self.state.last_tick_at(id);

        let mut candidate = definition
            .schedule
            .next_tick_at_or_after(now - lookback, &self.zone)?;

        loop {
            let unanswered = answered < Some(candidate);
            if candidate >= now {
                return if unanswered {
                    Some(candidate)
                } else {
                    definition.schedule.next_tick_after(candidate, &self.zone)
                };
            }
            // Behind us, but its jittered fire time may still be ahead.
            if unanswered && self.fire_at(id, definition, candidate) >= now {
                return Some(candidate);
            }
            candidate = definition.schedule.next_tick_after(candidate, &self.zone)?;
        }
    }

    /// When a Tick actually fires, once this Task's jitter is applied.
    fn fire_at(&self, id: &str, definition: &TaskDefinition, tick: DateTime<Utc>) -> DateTime<Utc> {
        crate::jitter::fire_at(
            &definition.schedule,
            id,
            definition.jitter,
            tick,
            &self.zone,
        )
    }

    /// Records the Ticks that passed while the Daemon wasn't running as one
    /// collapsed Skip. A month of missed minutely Ticks is one honest entry,
    /// not forty thousand rows nobody reads.
    fn record_downtime(&mut self, id: &str, definition: &TaskDefinition, now: DateTime<Utc>) {
        let Some(last) = self
            .state
            .last_tick_at(id)
            .or_else(|| self.state.last_scheduled_for(id))
        else {
            return;
        };

        let mut missed: Option<(DateTime<Utc>, DateTime<Utc>, usize)> = None;
        let mut truncated = false;
        let mut cursor = last;
        while let Some(tick) = definition.schedule.next_tick_after(cursor, &self.zone) {
            if tick >= now {
                break;
            }
            missed = Some(match missed {
                None => (tick, tick, 1),
                Some((from, _, count)) => (from, tick, count + 1),
            });
            cursor = tick;
            if missed.is_some_and(|(_, _, count)| count >= MAX_COUNTED_MISSES) {
                tracing::warn!(task = %id, "stopped counting missed ticks at {MAX_COUNTED_MISSES}");
                truncated = true;
                break;
            }
        }

        if let Some((from, to, count)) = missed {
            tracing::warn!(task = %id, %from, %to, count, "ticks missed while the daemon was down");
            self.state.note_tick(id, to);
            self.state.record_skip(
                id,
                Skip::DaemonDown {
                    from,
                    to,
                    count,
                    recorded_at: now,
                    truncated,
                },
            );
        }
    }

    /// Starts every Task whose Tick has come due and returns immediately. A
    /// Task that missed several Ticks runs once, not once per Tick — there is
    /// no catch-up.
    pub async fn tick(&mut self) -> Result<()> {
        let now = self.clock.now();
        self.running.retain(|_, handle| !handle.is_finished());

        let due: Vec<(usize, DateTime<Utc>)> = self
            .scheduled
            .iter()
            .enumerate()
            .filter_map(
                |(index, entry)| match (entry.next_fire_at, entry.next_tick) {
                    (Some(fire_at), Some(tick)) if fire_at <= now => Some((index, tick)),
                    _ => None,
                },
            )
            .collect();

        let mut answered_any = !due.is_empty();

        for (index, tick) in due {
            let id = self.scheduled[index].id.clone();
            self.advance(index, tick, now);
            self.state.note_tick(&id, tick);

            // A Task never runs concurrently with itself; the Tick it would
            // have answered is recorded rather than dropped.
            if self.running.contains_key(&id) {
                tracing::warn!(task = %id, scheduled_for = %tick, "skipped: previous run still going");
                self.state.record_skip(
                    &id,
                    Skip::Overlap {
                        scheduled_for: tick,
                        recorded_at: now,
                    },
                );
                continue;
            }

            match self.start_run(index, Some(tick)) {
                // `start_run` saves state itself once the Agent is away.
                Ok(()) => answered_any = false,
                Err(error) => tracing::error!(task = %id, "run failed to start: {error:#}"),
            }
        }

        if answered_any {
            self.state.save(&self.state_dir)?;
        }
        Ok(())
    }

    /// Moves a Task past the Tick it just answered.
    ///
    /// Walks forward one Tick at a time rather than jumping to `now`: if the
    /// scheduler fell behind — a suspended laptop, a pass that ran long — the
    /// Ticks in between are recorded rather than vanishing. Every Tick becomes
    /// exactly one Run or one Skip, and that has to hold when we are late.
    fn advance(&mut self, index: usize, answered: DateTime<Utc>, now: DateTime<Utc>) {
        let (id, definition) = {
            let entry = &self.scheduled[index];
            (entry.id.clone(), entry.definition.clone())
        };

        let mut cursor = answered;
        let mut overdue: Option<(DateTime<Utc>, DateTime<Utc>, usize)> = None;

        let pending = loop {
            let Some(tick) = definition.schedule.next_tick_after(cursor, &self.zone) else {
                break None;
            };
            if self.fire_at(&id, &definition, tick) > now {
                break Some(tick);
            }
            overdue = Some(match overdue {
                None => (tick, tick, 1),
                Some((from, _, count)) => (from, tick, count + 1),
            });
            cursor = tick;
        };

        if let Some((from, to, count)) = overdue {
            tracing::warn!(task = %id, %from, %to, count, "ticks passed before the scheduler reached them");
            self.state.note_tick(&id, to);
            self.state.record_skip(
                &id,
                Skip::Missed {
                    from,
                    to,
                    count,
                    recorded_at: now,
                },
            );
        }

        let entry = &mut self.scheduled[index];
        entry.next_tick = pending;
        entry.next_fire_at = pending.map(|tick| {
            crate::jitter::fire_at(
                &definition.schedule,
                &id,
                definition.jitter,
                tick,
                &self.zone,
            )
        });
    }

    /// Awaits every in-flight Run.
    pub async fn wait_for_running(&mut self) {
        for (_, handle) in std::mem::take(&mut self.running) {
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

        let handle = tokio::spawn(async move {
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
        });
        self.running.insert(task_id, handle);

        Ok(())
    }
}
