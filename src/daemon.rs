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
/// How stale a missed Tick may be and still be worth catching up on. Beyond
/// this it is history, not work anybody is still waiting for.
const CATCH_UP_WINDOW: chrono::Duration = chrono::Duration::days(7);

/// A Ready Task plus when it next comes due. Broken Tasks never get one of
/// these, which is how "Broken never fires" is enforced.
struct Scheduled {
    id: String,
    project_dir: PathBuf,
    definition: TaskDefinition,
    agent: String,
    /// The definition's digest, recorded when a Run starts.
    digest: String,
    /// What this Task is waiting for, if anything.
    pending: Option<Pending>,
}

/// A Tick that has not happened yet, and the moment it will actually fire.
/// The two always travel together: a Tick without its jittered fire time is
/// not a state the scheduler can be in.
#[derive(Debug, Clone, Copy)]
struct Pending {
    /// What the schedule promised, and what the Run records.
    tick: DateTime<Utc>,
    /// The Tick plus this Task's jitter offset.
    fire_at: DateTime<Utc>,
}

impl Scheduled {
    /// Where the Agent starts: the Project root, or `cwd:` resolved against
    /// it. Validated when the Task was scanned, so it exists.
    fn working_dir(&self) -> PathBuf {
        match &self.definition.cwd {
            Some(cwd) => self.project_dir.join(cwd),
            None => self.project_dir.clone(),
        }
    }
}

pub struct Daemon {
    /// Where the config was loaded from, so a rescan can pick up a Project
    /// registered or removed while the Daemon is running.
    config_path: Option<PathBuf>,
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
    /// Resolved once at construction: a bad value must fail loudly at
    /// startup, not silently at 2am when a Tick tries to use it.
    default_timeout: crate::task::Timeout,
    /// What was last said about each Task, so a rescan repeats nothing.
    announced: HashMap<String, Vec<String>>,
    /// Ticks missed while away that `catch_up` says should still run, found
    /// during the first scan and fired by the next pass.
    catching_up: HashMap<String, DateTime<Utc>>,
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
        let default_timeout = config.default_timeout()?;
        Ok(Self {
            config_path: None,
            default_timeout,
            config,
            clock,
            zone,
            state_dir,
            state: State::default(),
            scheduled: Vec::new(),
            broken_count: 0,
            running: HashMap::new(),
            announced: HashMap::new(),
            catching_up: HashMap::new(),
            has_scanned: false,
        })
    }

    /// Re-reads this config file on every rescan, so `add` and `remove`
    /// reach a running Daemon without a restart.
    pub fn watching_config(mut self, path: PathBuf) -> Self {
        self.config_path = Some(path);
        self
    }

    pub fn state_dir(&self) -> &PathBuf {
        &self.state_dir
    }

    /// The directories being watched for Task files.
    pub fn project_dirs(&self) -> Vec<PathBuf> {
        self.config
            .projects
            .iter()
            .map(|project| project.path.clone())
            .collect()
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

    /// Closes out Runs that were still going when a previous Daemon stopped.
    ///
    /// Only the holder of the single-daemon lock may call this: a record left
    /// `running` is evidence of a crash only when nobody else could be
    /// writing it.
    pub fn recover_interrupted_runs(&self) {
        let closed = run::close_abandoned_runs(&self.state_dir, self.clock.now());
        if closed > 0 {
            tracing::warn!("{closed} run(s) were interrupted when the daemon last stopped");
        }
    }

    /// Rescans every Project and recomputes when each Task next fires.
    pub async fn reload(&mut self) -> Result<()> {
        let now = self.clock.now();
        self.reload_config();
        self.state = State::load_and_quarantine(&self.state_dir, now)?;

        let scan = discovery::scan_all(&self.config);
        let present: std::collections::BTreeSet<String> =
            scan.tasks.iter().map(|task| task.id.clone()).collect();
        let borrowed: std::collections::BTreeSet<&str> =
            present.iter().map(String::as_str).collect();
        for id in self
            .state
            .prune_missing(&borrowed, &scan.reachable_projects)
        {
            tracing::info!(task = %id, "task file is gone; forgetting it");
        }
        drop(borrowed);

        // What each Task is already waiting for. A rescan happens every few
        // seconds now, so recomputing a pending Tick would quietly step over
        // it — only a Task whose definition actually changed is re-planned.
        let carried: HashMap<String, (String, Option<Pending>)> = self
            .scheduled
            .drain(..)
            .map(|entry| (entry.id, (entry.digest, entry.pending)))
            .collect();

        crate::crontab::write_all(&scan.tasks, &self.config);

        let mut scheduled = Vec::new();
        let mut broken = 0;

        for task in scan.tasks {
            self.state
                .upsert(&task.id, &task.path.display().to_string());

            // Never blocked, never quiet — but said once per change, not
            // once per scan: rescans are frequent, and a warning repeated
            // every half minute is noise nobody reads.
            let mut notes: Vec<String> = task
                .novelty(self.state.last_run_digest(&task.id))
                .note()
                .map(str::to_string)
                .into_iter()
                .chain(task.warnings().iter().cloned())
                .collect();
            if let TaskHealth::Broken { error, .. } = &task.health {
                notes.push(format!("broken, will not run: {error}"));
            }
            let unheard = self.announced.get(&task.id) != Some(&notes);
            if unheard {
                for note in &notes {
                    tracing::warn!(task = %task.id, "{note}");
                }
            }
            self.announced.insert(task.id.clone(), notes);

            match task.health {
                TaskHealth::Ready { definition, agent } => {
                    if !self.has_scanned {
                        self.record_downtime(&task.id, &definition, now);
                    }
                    let unchanged = carried
                        .get(&task.id)
                        .filter(|(digest, _)| digest == &task.digest);
                    let pending = match unchanged {
                        Some((_, pending)) => *pending,
                        None => self
                            .first_tick(&task.id, &definition, now)
                            .map(|tick| self.plan(&task.id, &definition, tick)),
                    };
                    scheduled.push(Scheduled {
                        digest: task.digest,
                        pending,
                        id: task.id,
                        project_dir: task.project_dir,
                        definition: *definition,
                        agent,
                    });
                }
                TaskHealth::Broken { .. } => {
                    // Counted here; announced above, alongside every other
                    // note this Task has, so one scan says each thing once.
                    broken += 1;
                }
            }
        }

        self.announced.retain(|id, _| present.contains(id));
        scheduled.sort_by(|a, b| a.id.cmp(&b.id));
        self.scheduled = scheduled;
        self.broken_count = broken;
        self.has_scanned = true;

        self.state.save(&self.state_dir)?;
        Ok(())
    }

    /// Picks up edits to the config file. A config that no longer parses is
    /// reported and ignored: the Daemon keeps running what it already knows
    /// rather than forgetting every Project over a typo.
    fn reload_config(&mut self) {
        let Some(path) = self.config_path.clone() else {
            return;
        };
        match Config::load(&path) {
            Ok(mut config) => {
                config.state_dir = self.config.state_dir.clone();
                self.config = config;
            }
            Err(error) => tracing::error!("keeping the previous config: {error:#}"),
        }
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
        // A One-shot that already answered its moment is Completed; only a
        // new moment in the file gives it something to do again.
        if let Some(moment) = definition.schedule.one_shot_at()
            && self.state.completed_for(id) == Some(moment)
        {
            return None;
        }

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

    /// The environment overrides a Run gets: the config's, then the Task's.
    /// Later entries win, and `env` applies them after the profile has run.
    fn run_environment(&self, definition: &TaskDefinition) -> Vec<(String, String)> {
        self.config
            .env
            .iter()
            .chain(definition.env.iter())
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect()
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
        // A One-shot has one moment, so its downtime question is simply
        // whether that moment went by unattended.
        if let Some(moment) = definition.schedule.one_shot_at() {
            if moment >= now || self.state.completed_for(id) == Some(moment) {
                return;
            }
            self.state.note_tick(id, moment);
            self.state.record_skip(
                id,
                Skip::DaemonDown {
                    from: moment,
                    to: moment,
                    count: 1,
                    recorded_at: now,
                    truncated: false,
                },
            );
            if definition.catch_up && now - moment <= CATCH_UP_WINDOW {
                self.catching_up.insert(id.to_string(), moment);
            } else {
                if let Some(task) = self.state.task_mut(id) {
                    task.completed_for = Some(moment);
                }
            }
            return;
        }

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
            // Asked for, and recent enough to still be what someone wanted:
            // the most recent miss runs, the rest stay recorded.
            if definition.catch_up && now - to <= CATCH_UP_WINDOW {
                self.catching_up.insert(id.to_string(), to);
            }
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

        let mut due: Vec<(usize, DateTime<Utc>)> = self
            .scheduled
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| match entry.pending {
                Some(pending) if pending.fire_at <= now => Some((index, pending.tick)),
                _ => None,
            })
            .collect();

        // Missed Ticks that `catch_up` earned a Run for, once each.
        for (index, entry) in self.scheduled.iter().enumerate() {
            if let Some(missed) = self.catching_up.get(&entry.id) {
                due.push((index, *missed));
            }
        }
        self.catching_up.clear();

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

        self.scheduled[index].pending = pending.map(|tick| self.plan(&id, &definition, tick));
    }

    /// How many Runs this Task keeps: its Project's setting, else the
    /// global one.
    fn retention_for(&self, task_id: &str) -> usize {
        task_id
            .split_once('/')
            .and_then(|(project, _)| {
                self.config
                    .projects
                    .iter()
                    .find(|candidate| candidate.resolved_name() == project)
            })
            .and_then(|project| project.max_runs_per_task)
            .unwrap_or_else(|| self.config.max_runs_per_task())
    }

    /// Pairs a Tick with the moment it will actually fire.
    fn plan(&self, id: &str, definition: &TaskDefinition, tick: DateTime<Utc>) -> Pending {
        Pending {
            tick,
            fire_at: self.fire_at(id, definition, tick),
        }
    }

    /// Awaits every in-flight Run.
    pub async fn wait_for_running(&mut self) {
        for (_, handle) in std::mem::take(&mut self.running) {
            let _ = handle.await;
        }
    }

    /// Starts one Run: records it as running, spawns the Agent under a login
    /// shell, and hands the waiting to a background task so the scheduler
    /// stays responsive.
    fn start_run(&mut self, index: usize, scheduled_for: Option<DateTime<Utc>>) -> Result<()> {
        let task = &self.scheduled[index];
        let task_id = task.id.clone();
        let digest = task.digest.clone();
        let prompt = task.definition.prompt.clone();
        let working_dir = task.working_dir();

        // The Agent was resolved and checked when the Task was scanned; a
        // Task pointing at a missing one is Broken and never reaches here.
        let agent_name = task.agent.clone();
        let template = self
            .config
            .agent(&agent_name)
            .with_context(|| format!("task {task_id} names unknown agent {agent_name:?}"))?
            .cmd
            .clone();

        let command = runner::login_shell_command(
            runner::build_command(
                &template,
                &runner::AgentParams {
                    prompt: &prompt,
                    model: task.definition.model.as_deref(),
                    permission_mode: task.definition.permission_mode.as_deref(),
                },
            )?,
            &self.run_environment(&task.definition),
        );

        let timeout = task.definition.timeout.unwrap_or(self.default_timeout);

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

        let log_path = run_dir.join(run::RUN_LOG);
        std::fs::write(&log_path, run::log_header(&record))
            .with_context(|| format!("writing {}", log_path.display()))?;

        // One pipe carries both streams, so stdout and stderr interleave in
        // their real order — and so the copy can stop at the cap without the
        // Agent noticing.
        let (reader, writer) = std::io::pipe().context("creating the output pipe")?;
        let mut process = tokio::process::Command::new(&command.program);
        process
            .args(&command.args)
            .current_dir(&working_dir)
            .stdout(Stdio::from(writer.try_clone()?))
            .stderr(Stdio::from(writer))
            .stdin(if command.prompt_on_stdin {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            // Its own process group, so a timeout can take the Agent's whole
            // tree of children with it rather than orphaning them.
            .process_group(0);

        let mut child = process
            .spawn()
            .with_context(|| format!("spawning agent for {task_id}"))?;
        let group = child.id().map(|pid| pid as i32);

        let cap = self.config.max_log_bytes();
        let capture =
            tokio::task::spawn_blocking(move || run::capture_output(reader, &log_path, cap));

        let completes = self.scheduled[index].definition.schedule.one_shot_at();
        if let Some(entry) = self.state.task_mut(&task_id) {
            entry.last_run_at = Some(started_at);
            entry.last_scheduled_for = scheduled_for;
            // Running it is what makes it familiar.
            entry.last_run_digest = Some(digest);
            // And, for a One-shot, what finishes it.
            entry.completed_for = completes;
        }
        self.state.save(&self.state_dir)?;

        let keep = self.retention_for(&task_id);
        match run::prune_runs(&self.state_dir, &task_id, keep) {
            Ok(0) => {}
            Ok(pruned) => {
                tracing::info!(task = %task_id, "pruned {pruned} old run(s), keeping {keep}")
            }
            Err(error) => tracing::warn!(task = %task_id, "pruning old runs: {error:#}"),
        }

        let clock = Arc::clone(&self.clock);
        let piped_prompt = command.prompt_on_stdin.then_some(prompt);

        let handle = tokio::spawn(async move {
            if let Some(prompt) = piped_prompt
                && let Some(mut stdin) = child.stdin.take()
                && let Err(error) = stdin.write_all(prompt.as_bytes()).await
            {
                tracing::warn!(task = %record.task_id, "writing prompt to stdin: {error}");
            }

            match await_child(&mut child, group, timeout).await {
                Outcome::Exited(status) => {
                    record.exit_code = status.code();
                    record.status = if status.success() {
                        RunStatus::Succeeded
                    } else {
                        RunStatus::Failed
                    };
                }
                Outcome::TimedOut => {
                    tracing::warn!(task = %record.task_id, "timed out; killed the process group");
                    record.status = RunStatus::TimedOut;
                }
                Outcome::Failed(error) => {
                    tracing::error!(task = %record.task_id, "waiting for agent: {error}");
                    record.status = RunStatus::Interrupted;
                }
            }

            // The log is complete once the pipe drains. A grandchild that
            // outlives the Agent can hold the write end open, so this waits
            // only so long — a Run must not stay "running" forever because
            // something it started refuses to let go.
            match tokio::time::timeout(CAPTURE_GRACE, capture).await {
                Ok(Ok(Ok(_))) => {}
                Ok(Ok(Err(error))) => {
                    tracing::error!(task = %record.task_id, "capturing output: {error:#}");
                }
                Ok(Err(error)) => {
                    tracing::error!(task = %record.task_id, "output capture failed: {error}");
                }
                Err(_) => tracing::warn!(
                    task = %record.task_id,
                    "output still open after the agent exited; something it started is holding it"
                ),
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

/// How a Run ended.
enum Outcome {
    Exited(std::process::ExitStatus),
    TimedOut,
    Failed(std::io::Error),
}

/// How long a signalled process group gets to wind down before it is killed.
const KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(10);
/// How long to keep draining output after the Agent itself has exited.
const CAPTURE_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Waits for the Agent, enforcing its timeout.
///
/// Timeouts are measured against real elapsed time, not the injected clock:
/// a Run's budget is about how long a process may spend, which is a different
/// question from when the schedule says it should start.
async fn await_child(
    child: &mut tokio::process::Child,
    group: Option<i32>,
    timeout: crate::task::Timeout,
) -> Outcome {
    let budget = match timeout {
        crate::task::Timeout::Never => None,
        crate::task::Timeout::After(duration) => duration.to_std().ok(),
    };

    let Some(budget) = budget else {
        return match child.wait().await {
            Ok(status) => Outcome::Exited(status),
            Err(error) => Outcome::Failed(error),
        };
    };

    tokio::select! {
        finished = child.wait() => match finished {
            Ok(status) => Outcome::Exited(status),
            Err(error) => Outcome::Failed(error),
        },
        _ = tokio::time::sleep(budget) => {
            terminate(child, group).await;
            Outcome::TimedOut
        }
    }
}

/// Ends a Run's whole process group: SIGTERM, a grace period, then SIGKILL.
async fn terminate(child: &mut tokio::process::Child, group: Option<i32>) {
    // Everything below signals the group *before* the leader is reaped. Once
    // the leader's pid is freed the kernel may hand it to someone else, and
    // signalling then would hit an unrelated process group.
    signal_group(group, libc::SIGTERM);

    if tokio::time::timeout(KILL_GRACE, child.wait()).await.is_ok() {
        return;
    }

    signal_group(group, libc::SIGKILL);
    let _ = child.wait().await;
}

fn signal_group(group: Option<i32>, signal: i32) {
    if let Some(group) = group {
        // Safety: `killpg` on a group we created; a dead group is simply ESRCH.
        unsafe {
            libc::killpg(group, signal);
        }
    }
}
