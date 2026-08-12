//! Cron expressions and the pure "when does this fire next?" computation.
//!
//! Standard 5-field crontab syntax plus the `@` aliases; no seconds field.
//! Every function here is pure: the caller supplies `now` and the zone.

use chrono::{DateTime, TimeZone, Utc};
use croner::Cron;
use croner::parser::{CronParser, Seconds};

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("invalid cron expression {expression:?}: {reason}")]
    InvalidCron { expression: String, reason: String },
}

/// When a Task comes due.
#[derive(Debug, Clone)]
pub enum Schedule {
    /// Repeats on a crontab expression. Boxed because a parsed cron is far
    /// larger than a timestamp, and most Tasks carry the schedule around by
    /// value.
    Cron(Box<CronSchedule>),
    /// Happens once, at a stated moment.
    At(DateTime<Utc>),
    /// Never on its own — only when Fired.
    Manual,
}

impl Schedule {
    /// How this schedule reads in a listing.
    pub fn expression(&self) -> String {
        match self {
            Schedule::Cron(cron) => cron.expression.clone(),
            Schedule::At(when) => when.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            Schedule::Manual => "no schedule".to_string(),
        }
    }

    /// The first Tick strictly after `after`.
    pub fn next_tick_after<Tz: TimeZone>(
        &self,
        after: DateTime<Utc>,
        zone: &Tz,
    ) -> Option<DateTime<Utc>> {
        match self {
            Schedule::Cron(cron) => cron.find(after, zone, false),
            Schedule::At(when) => (*when > after).then_some(*when),
            Schedule::Manual => None,
        }
    }

    /// The first Tick at or after `at`.
    pub fn next_tick_at_or_after<Tz: TimeZone>(
        &self,
        at: DateTime<Utc>,
        zone: &Tz,
    ) -> Option<DateTime<Utc>> {
        match self {
            Schedule::Cron(cron) => cron.find(at, zone, true),
            Schedule::At(when) => (*when >= at).then_some(*when),
            Schedule::Manual => None,
        }
    }

    /// The gap from `tick` to the Tick after it, if there is one.
    pub fn interval_after<Tz: TimeZone>(
        &self,
        tick: DateTime<Utc>,
        zone: &Tz,
    ) -> Option<chrono::Duration> {
        self.next_tick_after(tick, zone)
            .map(|following| following - tick)
    }

    /// Whether this Task ever comes due on its own.
    pub fn is_manual(&self) -> bool {
        matches!(self, Schedule::Manual)
    }

    /// The moment a One-shot is waiting for.
    pub fn one_shot_at(&self) -> Option<DateTime<Utc>> {
        match self {
            Schedule::At(when) => Some(*when),
            _ => None,
        }
    }
}

/// A parsed crontab expression.
#[derive(Debug, Clone)]
pub struct CronSchedule {
    expression: String,
    cron: Cron,
}

impl CronSchedule {
    pub fn parse(expression: &str) -> Result<Self, ScheduleError> {
        let expression = expression.trim();

        // Seconds are disallowed so the accepted dialect is exactly the
        // 5-field crontab syntax the docs promise — no sub-minute surprises.
        let cron = CronParser::builder()
            .seconds(Seconds::Disallowed)
            .build()
            .parse(expression)
            .map_err(|error| ScheduleError::InvalidCron {
                expression: expression.to_string(),
                reason: error.to_string(),
            })?;

        Ok(Self {
            expression: expression.to_string(),
            cron,
        })
    }

    fn find<Tz: TimeZone>(
        &self,
        from: DateTime<Utc>,
        zone: &Tz,
        inclusive: bool,
    ) -> Option<DateTime<Utc>> {
        let local = from.with_timezone(zone);
        self.cron
            .find_next_occurrence(&local, inclusive)
            .ok()
            .map(|next| next.with_timezone(&Utc))
    }
}
