//! The timezone Ticks are evaluated in.
//!
//! Cron runs in the host's local zone — what a crontab user expects — but the
//! zone is resolved once and carried explicitly rather than read from ambient
//! state, so DST behaviour is observable and testable.

use chrono_tz::Tz;

/// The host's IANA zone, falling back to UTC when it can't be determined.
pub fn host() -> Tz {
    iana_time_zone::get_timezone()
        .ok()
        .and_then(|name| name.parse::<Tz>().ok())
        .unwrap_or(Tz::UTC)
}
