//! Spreading fire times so a machine full of midnight Tasks doesn't stampede
//! whatever the Agents talk to.
//!
//! The offset is derived from the Task id, so it is stable: the same Task
//! always lands at the same point in its window, run after run and restart
//! after restart. Nothing here is random.

use crate::schedule::Schedule;
use chrono::{DateTime, Duration, TimeZone, Utc};

/// The widest a Task's fire time is ever nudged, absent a per-task setting.
pub const DEFAULT_WINDOW: Duration = Duration::minutes(5);

/// The window a Task's offset is drawn from: the configured amount (or the
/// default), never more than half the gap to its next Tick — a Task running
/// every two minutes must not be nudged by five.
pub fn window(interval: Option<Duration>, configured: Option<Duration>) -> Duration {
    let requested = configured.unwrap_or(DEFAULT_WINDOW);
    match interval {
        Some(interval) if interval > Duration::zero() => requested.min(interval / 2),
        _ => requested,
    }
}

/// When a Task whose Tick is `tick` actually fires: the Tick nudged by this
/// Task's own offset.
///
/// `list` deliberately shows the Tick rather than this — a user comparing the
/// row against their own cron expression should see the time they wrote.
pub fn fire_at<Tz: TimeZone>(
    schedule: &Schedule,
    task_id: &str,
    configured: Option<Duration>,
    tick: DateTime<Utc>,
    zone: &Tz,
) -> DateTime<Utc> {
    let interval = schedule.interval_after(tick, zone);
    tick + offset(task_id, window(interval, configured))
}

/// This Task's offset into `window`. Deterministic in the Task id.
pub fn offset(task_id: &str, window: Duration) -> Duration {
    let seconds = window.num_seconds();
    if seconds <= 0 {
        return Duration::zero();
    }
    Duration::seconds((crate::digest::fnv1a(task_id.as_bytes()) % seconds as u64) as i64)
}
