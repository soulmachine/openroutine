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

#[derive(Debug, Clone)]
pub struct Schedule {
    expression: String,
    cron: Cron,
}

impl Schedule {
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

    /// The expression exactly as the task file wrote it.
    pub fn expression(&self) -> &str {
        &self.expression
    }

    /// The first tick strictly after `after`, evaluated in `zone`.
    pub fn next_fire_after<Tz: TimeZone>(
        &self,
        after: DateTime<Utc>,
        zone: &Tz,
    ) -> Option<DateTime<Utc>> {
        let local = after.with_timezone(zone);
        self.cron
            .find_next_occurrence(&local, false)
            .ok()
            .map(|next| next.with_timezone(&Utc))
    }
}
