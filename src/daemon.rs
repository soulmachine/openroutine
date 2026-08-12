//! The Daemon: scheduler and runner in one process.
//!
//! Time comes only from the injected [`Clock`], so the whole schedule can be
//! driven deterministically. Every due Tick becomes exactly one Run or one
//! recorded Skip, and a Run never blocks the scheduler — it is started, then
//! awaited elsewhere.

use crate::clock::Clock;
use crate::config::Config;
use crate::registry::{self, TaskHealth};
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
    path: PathBuf,
    /// The directory holding the file, which a relative `cwd:` resolves
    /// against and which a Run falls back to.
    dir: PathBuf,
    definition: TaskDefinition,
    agent: String,
    /// The definition's digest, recorded when a Run starts.
    digest: String,
    /// The file's mtime when it was last read. In memory only: a Refresh uses
    /// it to skip reading a file nothing has touched, and a restart re-reads
    /// everything anyway.
    mtime: Option<std::time::SystemTime>,
    /// What this Task is waiting for, if anything.
    pending: Option<Pending>,
}

/// What a Refresh made of a Task's file, moments before it would have run.
enum Refreshed {
    /// The definition on disk is the one already loaded.
    Unchanged,
    /// A new definition was adopted and is what should now run.
    Adopted,
    /// The file changed into something that cannot run at all — it no longer
    /// loads, it switched itself off, or it renamed itself. The Run does not
    /// happen and the Task holds no schedule until a Reload says what it has
    /// become. The string is why, in the words the Skip records.
    Withdrawn(String),
    /// The Task is still runnable, but not for *this* Tick: a One-shot whose
    /// moment moved before it arrived. The Run does not happen, and the Task
    /// stays scheduled — already re-armed at the moment it now names.
    Rearmed(String),
}

/// A Run in flight, and what is needed to stop it.
struct RunningRun {
    handle: JoinHandle<()>,
    run_id: String,
    group: Option<i32>,
}

/// What happened when a Fire was asked for.
#[derive(Debug, Clone)]
pub enum FireOutcome {
    Started(String),
    AlreadyRunning,
    Paused,
    Disabled,
    NoSuchTask,
    Failed(String),
}

/// What happened when a cancellation was asked for.
#[derive(Debug, Clone, Copy)]
pub enum CancelOutcome {
    Cancelled,
    NotRunning,
}

/// What a Reload made of one Task.
#[derive(Debug, Clone)]
pub struct ReloadEntry {
    pub id: String,
    pub path: String,
    pub status: ReloadStatus,
    /// Why it cannot run, when it cannot.
    pub error: Option<String>,
    pub warnings: Vec<String>,
    /// Set when this Task is new or has changed since it last ran.
    pub novelty: Option<&'static str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadStatus {
    Ready,
    Disabled,
    Broken,
}

impl ReloadStatus {
    pub fn label(self) -> &'static str {
        match self {
            ReloadStatus::Ready => "ready",
            ReloadStatus::Disabled => "disabled",
            ReloadStatus::Broken => "broken",
        }
    }
}

/// A scheduled Task as seen from outside.
#[derive(Debug, Clone)]
pub struct ScheduledSummary {
    pub id: String,
    pub description: String,
    /// Set when this Task is new or has changed since it last ran.
    pub novelty: Option<&'static str>,
    pub schedule: String,
    pub path: String,
    pub one_shot: bool,
    pub next_tick: Option<DateTime<Utc>>,
    pub next_fire_at: Option<DateTime<Utc>>,
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

pub struct Daemon {
    /// Where the config was loaded from, so a Reload can pick up a Task
    /// registered or removed while the Daemon is running.
    config_path: Option<PathBuf>,
    config: Config,
    clock: Arc<dyn Clock>,
    zone: Tz,
    state_dir: PathBuf,
    state: State,
    scheduled: Vec<Scheduled>,
    /// Broken Tasks seen by the last scan: id, error, and whatever
    /// description survived. Reported, never scheduled.
    broken: Vec<(String, String, Option<String>)>,
    /// Tasks switched off in their own files: id and description. Reported,
    /// never scheduled.
    disabled: Vec<(String, String)>,
    /// Runs started and not yet finished, by Task id — a Task appearing here
    /// is why its next Tick becomes an overlap Skip.
    running: HashMap<String, RunningRun>,
    /// Resolved once at construction: a bad value must fail loudly at
    /// startup, not silently at 2am when a Tick tries to use it.
    idle_timeout: crate::task::Timeout,
    /// What was last said about each Task file, so a rescan repeats nothing.
    /// Keyed by path rather than name: two files can claim one name, and only
    /// one of them can win it.
    announced: HashMap<String, Vec<String>>,
    /// Ticks missed while away that `catch_up` says should still run, found
    /// during the first scan and fired by the next pass.
    catching_up: HashMap<String, DateTime<Utc>>,
    /// Whether this Daemon has loaded yet. Downtime is a startup question:
    /// only the first load can tell missed Ticks from ordinary ones.
    has_scanned: bool,
    /// Why the config could not be re-read at the last Reload, if it could
    /// not. The previous config stays in force; this says so out loud.
    config_error: Option<String>,
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
        let idle_timeout = config.idle_timeout()?;
        Ok(Self {
            config_path: None,
            idle_timeout,
            config,
            clock,
            zone,
            state_dir,
            state: State::default(),
            scheduled: Vec::new(),
            broken: Vec::new(),
            disabled: Vec::new(),
            running: HashMap::new(),
            announced: HashMap::new(),
            catching_up: HashMap::new(),
            has_scanned: false,
            config_error: None,
        })
    }

    /// Re-reads this config file on every Reload, so `add` and `remove`
    /// reach a running Daemon without a restart.
    pub fn watching_config(mut self, path: PathBuf) -> Self {
        self.config_path = Some(path);
        self
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
        self.broken.len()
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

    /// Re-reads the config and every registered Task file, and recomputes
    /// when each Task next fires.
    ///
    /// The only moment definitions change: nothing is watched, and no Tick
    /// re-reads anything. Returns what it found, Task by Task, so whoever
    /// asked for the Reload learns what it did.
    pub async fn reload(&mut self) -> Result<Vec<ReloadEntry>> {
        let now = self.clock.now();
        let config_error = self.reload_config();
        self.state = State::load_and_quarantine(&self.state_dir, now)?;

        let loaded = registry::load_all(&self.config);
        let present: std::collections::BTreeSet<String> =
            loaded.iter().map(|task| task.id.clone()).collect();
        // Announcements are per file, not per name: during a name collision
        // two files answer to one id, and keying by id would let each
        // overwrite the other's notes and re-warn on every Reload.
        let present_files: std::collections::BTreeSet<String> = loaded
            .iter()
            .map(|task| task.path.display().to_string())
            .collect();
        let borrowed: std::collections::BTreeSet<&str> =
            present.iter().map(String::as_str).collect();
        for id in self.state.prune_missing(&borrowed) {
            tracing::info!(task = %id, "task is no longer registered; forgetting it");
        }
        drop(borrowed);

        // What each Task is already waiting for. Recomputing a pending Tick
        // would quietly step over it, so only a Task whose definition
        // actually changed is re-planned.
        let carried: HashMap<String, (String, Option<Pending>)> = self
            .scheduled
            .drain(..)
            .map(|entry| (entry.id, (entry.digest, entry.pending)))
            .collect();

        let mut report = Vec::new();
        let mut scheduled = Vec::new();
        let mut broken = Vec::new();
        let mut disabled = Vec::new();

        for task in loaded {
            let working_dir = task
                .definition()
                .map(|definition| definition.working_dir(&task.dir))
                .unwrap_or_else(|| task.dir.clone());
            // A file that lost a name collision writes nothing: the state row
            // under that name — its history, its digest, its Skips — belongs
            // to the file that claimed the name first, and letting the loser
            // upsert would file the winner's past under the wrong path.
            if !task.duplicate {
                self.state.upsert(
                    &task.id,
                    &task.path.display().to_string(),
                    &working_dir.display().to_string(),
                );
            }

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
            let file = task.path.display().to_string();
            let unheard = self.announced.get(&file) != Some(&notes);
            if unheard {
                for note in &notes {
                    tracing::warn!(task = %task.id, "{note}");
                }
            }
            report.push(ReloadEntry {
                id: task.id.clone(),
                path: task.path.display().to_string(),
                status: match &task.health {
                    TaskHealth::Broken { .. } => ReloadStatus::Broken,
                    TaskHealth::Ready { definition, .. } if definition.disabled => {
                        ReloadStatus::Disabled
                    }
                    TaskHealth::Ready { .. } => ReloadStatus::Ready,
                },
                error: match &task.health {
                    TaskHealth::Broken { error, .. } => Some(error.clone()),
                    TaskHealth::Ready { .. } => None,
                },
                warnings: task.warnings().to_vec(),
                novelty: task.novelty(self.state.last_run_digest(&task.id)).note(),
            });
            self.announced.insert(file, notes);

            match task.health {
                TaskHealth::Ready { definition, agent } if definition.disabled => {
                    // Off in its own file: not scheduled, so no Ticks come
                    // due and nothing accumulates. Still listed, still
                    // visible — just not going to run.
                    disabled.push((task.id.clone(), definition.description.clone()));
                    let _ = agent;
                }
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
                            .first_tick(&task.id, &definition, &task.digest, now)
                            .map(|tick| self.plan(&task.id, &definition, tick)),
                    };
                    scheduled.push(Scheduled {
                        digest: task.digest,
                        mtime: task.mtime,
                        pending,
                        path: task.path,
                        id: task.id,
                        dir: task.dir,
                        definition: *definition,
                        agent,
                    });
                }
                TaskHealth::Broken { error, description } => {
                    // Kept whole, not counted: a Task nobody can run is
                    // exactly the one somebody needs to see.
                    broken.push((task.id.clone(), error, description));
                }
            }
        }

        self.announced
            .retain(|file, _| present_files.contains(file));
        scheduled.sort_by(|a, b| a.id.cmp(&b.id));
        self.scheduled = scheduled;
        broken.sort();
        self.broken = broken;
        disabled.sort();
        self.disabled = disabled;
        self.has_scanned = true;

        self.state.save(&self.state_dir)?;
        self.config_error = config_error;
        report.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(report)
    }

    /// What the last Reload made of the config file, if it could not read it.
    pub fn config_error(&self) -> Option<&str> {
        self.config_error.as_deref()
    }

    /// Picks up edits to the config file. A config that no longer parses is
    /// reported and ignored: the Daemon keeps running what it already knows
    /// rather than forgetting every Task over a typo.
    fn reload_config(&mut self) -> Option<String> {
        let path = self.config_path.clone()?;
        match Config::load(&path) {
            Ok(mut config) => {
                config.state_dir = self.config.state_dir.clone();
                self.config = config;
                None
            }
            Err(error) => {
                tracing::error!("keeping the previous config: {error:#}");
                Some(format!("{error:#}"))
            }
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
        digest: &str,
        now: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        // A One-shot with no moment of its own runs as soon as the Daemon
        // takes it in, and is then Completed. Its digest is what re-arms it:
        // the definition that last ran is the one already answered, so an
        // edit — and only an edit — gives it something to do again.
        if matches!(definition.schedule, crate::schedule::Schedule::Once) {
            return match self.state.last_run_digest(id) {
                Some(ran) if ran == digest => None,
                _ => Some(now),
            };
        }

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

    /// Starts every Task whose Tick has come due and returns immediately.
    ///
    /// A Task that missed several Ticks runs once, not once per Tick: the
    /// rest are recorded as Skips. Only `catch_up: true` asks for one of
    /// those missed Ticks to be run after the fact.
    pub async fn tick(&mut self) -> Result<()> {
        let now = self.clock.now();
        self.running.retain(|_, run| !run.handle.is_finished());

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

        let answered_any = !due.is_empty();
        // Tasks whose file changed out from under them mid-pass. Collected
        // rather than removed on the spot: `due` holds indices into
        // `self.scheduled`, and removing one would invalidate the rest.
        let mut withdrawn: Vec<usize> = Vec::new();

        for (index, tick) in due {
            let id = self.scheduled[index].id.clone();

            // At capacity: the Tick waits for a slot rather than being lost.
            // Its pending stays exactly where it is, so the next pass finds
            // it still due, and the Run records how late it ended up.
            if self.at_capacity() && !self.running.contains_key(&id) {
                tracing::debug!(task = %id, scheduled_for = %tick, "waiting for a free slot");
                continue;
            }

            self.advance(index, tick, now);
            self.state.note_tick(&id, tick);

            // Held, by this Task's own pause or by the global one. The Tick
            // is still answered — with a Skip — rather than disappearing.
            if self.state.is_paused(&id) {
                tracing::debug!(task = %id, scheduled_for = %tick, "skipped: paused");
                self.state.record_skip(
                    &id,
                    Skip::Paused {
                        scheduled_for: tick,
                        recorded_at: now,
                    },
                );
                continue;
            }

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

            // Last thing before the Agent starts: the file may have been
            // fixed, switched off, or broken since this Tick was planned.
            // After the pause and overlap checks, so a Tick that was never
            // going to run does no I/O.
            // Withdrawn cannot run in the form it now has, so a Reload has to
            // say what it became. Rearmed is still a Task — just not for this
            // Tick — and keeps the schedule the Refresh gave it.
            let (skipped, withdraw) = match self.refresh(index, Some(tick)) {
                Refreshed::Unchanged => (None, false),
                Refreshed::Adopted => {
                    tracing::info!(task = %id, "definition changed since it was loaded; running the new one");
                    (None, false)
                }
                Refreshed::Withdrawn(why) => (Some(why), true),
                Refreshed::Rearmed(why) => (Some(why), false),
            };
            if let Some(why) = skipped {
                tracing::warn!(task = %id, scheduled_for = %tick, "skipped: {why}");
                self.state.record_skip(
                    &id,
                    Skip::DefinitionChanged {
                        scheduled_for: tick,
                        recorded_at: now,
                        detail: why,
                    },
                );
                if withdraw {
                    withdrawn.push(index);
                }
                continue;
            }

            if let Err(error) = self.start_run(index, Some(tick), None) {
                tracing::error!(task = %id, "run failed to start: {error:#}");
            }
        }

        // Removed after the pass, highest index first, so the indices the
        // loop was holding stayed valid while it ran.
        withdrawn.sort_unstable();
        withdrawn.dedup();
        for index in withdrawn.into_iter().rev() {
            let entry = self.scheduled.remove(index);
            self.broken.push((
                entry.id,
                "withdrawn at its fire time; run `openroutine reload`".to_string(),
                Some(entry.definition.description),
            ));
        }
        self.broken.sort();

        // One save for the whole pass, after every Tick in it has been
        // answered. Saving as soon as the first Run started used to lose the
        // Skips recorded by the Ticks behind it.
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

    /// How many Runs each Task keeps.
    fn retention_for(&self) -> usize {
        self.config.max_runs_per_task()
    }

    /// Re-reads one Task's own file, moments before it would run.
    ///
    /// The only place a definition changes outside a Reload, and deliberately
    /// the narrowest one: this file, on this path, for this Run. Nothing is
    /// watched, no timer fires, no other Task is touched, and the config is
    /// not re-read — registering and unregistering stay a Reload's business.
    ///
    /// mtime is only a pre-filter. A file whose mtime moved is read and
    /// digested, and only a changed digest counts: `git checkout` and `touch`
    /// move mtime with identical content, and treating that as a change would
    /// re-arm every cron-less One-shot on a branch switch.
    fn refresh(&mut self, index: usize, tick: Option<DateTime<Utc>>) -> Refreshed {
        let (path, known_mtime, known_digest) = {
            let entry = &self.scheduled[index];
            (entry.path.clone(), entry.mtime, entry.digest.clone())
        };

        let mtime = std::fs::metadata(&path)
            .and_then(|meta| meta.modified())
            .ok();
        // Absent on either side means "cannot tell", which is a reason to
        // read, not a reason to assume.
        if let (Some(now), Some(then)) = (mtime, known_mtime)
            && now == then
        {
            return Refreshed::Unchanged;
        }

        let loaded = registry::load_one(&path, &self.config);
        self.scheduled[index].mtime = loaded.mtime;
        if loaded.digest == known_digest {
            // Touched, not edited.
            return Refreshed::Unchanged;
        }

        let (definition, agent) = match loaded.health {
            TaskHealth::Ready { definition, agent } => (*definition, agent),
            TaskHealth::Broken { error, .. } => {
                return Refreshed::Withdrawn(format!("its definition no longer loads: {error}"));
            }
        };
        if definition.disabled {
            return Refreshed::Withdrawn("it was disabled in its own file".to_string());
        }
        // Identity is a registry-level change: a new id can collide with
        // another registration and would strand this one's run history, and
        // the fire path is the wrong place to settle that.
        if definition.id != self.scheduled[index].id {
            return Refreshed::Withdrawn(format!(
                "it renamed itself to {:?}; run `openroutine reload`",
                definition.id
            ));
        }

        // A One-shot's moment is its identity. If the author moved it before
        // it arrived, firing at the old one would be plainly wrong — so the
        // Tick is withdrawn and the Task re-armed at the moment it now names.
        let moment_moved = tick.is_some_and(|tick| {
            definition
                .schedule
                .one_shot_at()
                .is_some_and(|moment| moment != tick)
        });

        let entry = &mut self.scheduled[index];
        entry.digest = loaded.digest;
        entry.definition = definition;
        entry.agent = agent;
        entry.dir = loaded.dir;

        if moment_moved {
            let (id, definition) = (entry.id.clone(), entry.definition.clone());
            let now = self.clock.now();
            self.scheduled[index].pending = self
                .first_tick(&id, &definition, &self.scheduled[index].digest.clone(), now)
                .map(|tick| self.plan(&id, &definition, tick));
            return Refreshed::Rearmed("its moment moved before it arrived".to_string());
        }

        Refreshed::Adopted
    }

    /// Pairs a Tick with the moment it will actually fire.
    fn plan(&self, id: &str, definition: &TaskDefinition, tick: DateTime<Utc>) -> Pending {
        Pending {
            tick,
            fire_at: self.fire_at(id, definition, tick),
        }
    }

    /// Stops a Run that is in flight.
    ///
    /// Ends the whole process group, the same way a timeout does, so the
    /// Agent's children go with it.
    pub fn cancel(&mut self, task_id: &str, run_id: &str) -> CancelOutcome {
        self.running.retain(|_, run| !run.handle.is_finished());
        match self.running.get(task_id) {
            Some(run) if run.run_id == run_id => {
                signal_group(run.group, libc::SIGTERM);
                CancelOutcome::Cancelled
            }
            Some(_) | None => CancelOutcome::NotRunning,
        }
    }

    /// Holds or releases one Task, or every Task at once.
    pub fn set_paused(&mut self, task: Option<&str>, paused: bool) -> Result<()> {
        match task {
            Some(id) => {
                self.state.set_paused(id, paused);
            }
            None => self.state.paused = paused,
        }
        self.state.save(&self.state_dir)
    }

    /// Whether everything is held.
    pub fn is_paused(&self, task: Option<&str>) -> bool {
        match task {
            Some(id) => self.state.is_paused(id),
            None => self.state.paused,
        }
    }

    /// Starts a Run now, outside the schedule.
    ///
    /// A Task never runs concurrently with itself, so a Fire arriving while
    /// one is going is refused with the Run already in flight rather than
    /// quietly queued.
    pub fn fire(&mut self, task_id: &str, context: Option<&str>) -> FireOutcome {
        self.running.retain(|_, run| !run.handle.is_finished());

        let Some(index) = self.scheduled.iter().position(|entry| entry.id == task_id) else {
            // Switched off in its file, so it is not scheduled — and firing
            // must not override what the file says.
            return if self.disabled.iter().any(|(id, _)| id == task_id) {
                FireOutcome::Disabled
            } else {
                FireOutcome::NoSuchTask
            };
        };
        if self.running.contains_key(task_id) {
            return FireOutcome::AlreadyRunning;
        }
        if self.state.is_paused(task_id) {
            return FireOutcome::Paused;
        }

        // A Fire is a Run about to start, so it Refreshes like any other —
        // "I edited it, then fired it" is the commonest way to meet this at
        // all. There is no Tick here, so a One-shot's moment cannot be the
        // thing that moved.
        match self.refresh(index, None) {
            Refreshed::Unchanged => {}
            Refreshed::Adopted => {
                tracing::info!(task = %task_id, "definition changed since it was loaded; firing the new one");
            }
            // No Tick was passed, so `refresh` has no moment to compare against
            // and never re-arms; only a Withdrawal can come back.
            Refreshed::Rearmed(why) | Refreshed::Withdrawn(why) => {
                tracing::warn!(task = %task_id, "refused to fire: {why}");
                let entry = self.scheduled.remove(index);
                let disabled = entry.definition.disabled;
                let description = entry.definition.description;
                if disabled {
                    self.disabled.push((entry.id, description));
                    self.disabled.sort();
                    return FireOutcome::Disabled;
                }
                self.broken.push((
                    entry.id,
                    format!("withdrawn at its fire time: {why}"),
                    Some(description),
                ));
                self.broken.sort();
                return FireOutcome::Failed(why);
            }
        }

        match self.start_run(index, None, context) {
            Ok(run_id) => {
                if let Err(error) = self.state.save(&self.state_dir) {
                    tracing::error!(task = %task_id, "recording the fired run: {error:#}");
                }
                FireOutcome::Started(run_id)
            }
            Err(error) => FireOutcome::Failed(format!("{error:#}")),
        }
    }

    /// Whether every slot is taken.
    fn at_capacity(&self) -> bool {
        self.config
            .max_parallel()
            .is_some_and(|limit| self.running.len() >= limit)
    }

    /// Whether this Task has a Run in flight.
    pub fn is_running(&self, task_id: &str) -> bool {
        self.running
            .get(task_id)
            .is_some_and(|run| !run.handle.is_finished())
    }

    /// Every Task currently scheduled, with what it is waiting for.
    pub fn scheduled_tasks(&self) -> Vec<ScheduledSummary> {
        self.scheduled
            .iter()
            .map(|entry| ScheduledSummary {
                novelty: match self.state.last_run_digest(&entry.id) {
                    None => Some("new"),
                    Some(seen) if seen != entry.digest => Some("changed"),
                    Some(_) => None,
                },
                id: entry.id.clone(),
                description: entry.definition.description.clone(),
                schedule: entry.definition.schedule.expression(),
                path: entry.path.display().to_string(),
                one_shot: entry.definition.schedule.is_one_shot(),
                next_tick: entry.pending.map(|pending| pending.tick),
                next_fire_at: entry.pending.map(|pending| pending.fire_at),
            })
            .collect()
    }

    /// Tasks the last scan could not use, so they can be shown rather than
    /// merely counted.
    pub fn broken_tasks(&self) -> Vec<(String, String, Option<String>)> {
        self.broken.clone()
    }

    /// Tasks switched off in their own files.
    pub fn disabled_tasks(&self) -> Vec<(String, String)> {
        self.disabled.clone()
    }

    /// The run state as the Daemon currently holds it.
    pub fn state(&self) -> &State {
        &self.state
    }

    /// Awaits every in-flight Run.
    pub async fn wait_for_running(&mut self) {
        for (_, run) in std::mem::take(&mut self.running) {
            let _ = run.handle.await;
        }
    }

    /// Starts one Run: records it as running, spawns the Agent under a login
    /// shell, and hands the waiting to a background task so the scheduler
    /// stays responsive.
    fn start_run(
        &mut self,
        index: usize,
        scheduled_for: Option<DateTime<Utc>>,
        context: Option<&str>,
    ) -> Result<String> {
        let task = &self.scheduled[index];
        let task_id = task.id.clone();
        let digest = task.digest.clone();
        let prompt = match context {
            Some(text) => crate::fire::with_context(&task.definition.prompt, text),
            None => task.definition.prompt.clone(),
        };
        let working_dir = task.definition.working_dir(&task.dir);

        // The Agent was resolved and checked when the Task was loaded; a
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
            &task.definition.environment(&self.config.env),
        );

        let idle_timeout = self.idle_timeout;

        let started_at = self.clock.now();
        let (run_id, run_dir) = run::create_run_dir(&self.state_dir, &task_id, started_at)?;

        let announced_id = run_id.clone();
        let mut record = RunRecord {
            run_id,
            task_id: task_id.clone(),
            status: RunStatus::Running,
            exit_code: None,
            trigger: if scheduled_for.is_some() {
                Trigger::Schedule
            } else {
                Trigger::Fire
            },
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
            // Seed the login shell the way a service manager would, rather
            // than passing on whatever this process holds. Under launchd the
            // two are already the same; started by `serve` from a terminal
            // they are not, and without this a Task would pass `list` and
            // run green all day in the foreground, then fail the first night
            // it ran installed. A profile that appends to `PATH` still gets
            // the last word, and so does a Task's own `env: PATH`.
            .env("PATH", runner::SERVICE_PATH)
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

        // The Run's own stopwatch. Real elapsed time, shared by the thread
        // draining the output and the task supervising the Agent, so the two
        // agree on how long it has been quiet.
        let running_since = std::time::Instant::now();
        let activity = run::Activity::default();

        let cap = self.config.max_log_bytes();
        let capture = {
            let activity = activity.clone();
            tokio::task::spawn_blocking(move || {
                run::capture_output(reader, &log_path, cap, running_since, activity)
            })
        };

        let completes = self.scheduled[index].definition.schedule.one_shot_at();
        if let Some(entry) = self.state.task_mut(&task_id) {
            entry.last_run_at = Some(started_at);
            entry.last_scheduled_for = scheduled_for;
            // Running it is what makes it familiar.
            entry.last_run_digest = Some(digest);
            // And, for a One-shot, what finishes it.
            entry.completed_for = completes;
        }

        let keep = self.retention_for();
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

            match await_child(&mut child, group, idle_timeout, running_since, &activity).await {
                Outcome::Exited(status) => {
                    record.exit_code = status.code();
                    record.status = if status.success() {
                        RunStatus::Succeeded
                    } else {
                        RunStatus::Failed
                    };
                }
                Outcome::TimedOut => {
                    tracing::warn!(
                        task = %record.task_id,
                        "went quiet for too long; killed the process group"
                    );
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
        self.running.insert(
            task_id,
            RunningRun {
                handle,
                run_id: announced_id.clone(),
                group,
            },
        );

        Ok(announced_id)
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

/// Waits for the Agent, ending it if it goes quiet for too long.
///
/// The budget is measured from the Agent's last output, not from the start of
/// the Run: work that is visibly progressing is never interrupted, however
/// long it takes, while a Run that has hung — a stuck network read, a tool
/// waiting on input nobody will type — is reaped instead of held forever.
///
/// Measured against real elapsed time, not the injected clock: how long a
/// process has been silent is a different question from when the schedule
/// said it should start.
async fn await_child(
    child: &mut tokio::process::Child,
    group: Option<i32>,
    idle_timeout: crate::task::Timeout,
    started: std::time::Instant,
    activity: &run::Activity,
) -> Outcome {
    let budget = match idle_timeout {
        crate::task::Timeout::Never => None,
        crate::task::Timeout::After(duration) => duration.to_std().ok(),
    };

    let Some(budget) = budget else {
        return match child.wait().await {
            Ok(status) => Outcome::Exited(status),
            Err(error) => Outcome::Failed(error),
        };
    };

    loop {
        // Sleep only as far as the current deadline. If the Agent spoke while
        // we waited, the deadline has moved and the next pass sleeps again —
        // so there is no polling interval to tune, and no drift.
        let quiet_for = activity.quiet_for(started.elapsed());
        let Some(remaining) = budget.checked_sub(quiet_for) else {
            terminate(child, group).await;
            return Outcome::TimedOut;
        };

        tokio::select! {
            finished = child.wait() => return match finished {
                Ok(status) => Outcome::Exited(status),
                Err(error) => Outcome::Failed(error),
            },
            _ = tokio::time::sleep(remaining) => continue,
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
