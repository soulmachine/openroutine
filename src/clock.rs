//! The single time source. Nothing in the daemon reads the wall clock
//! directly — every `now` comes from here, so scheduling is testable.

use chrono::{DateTime, Utc};
use std::sync::{Arc, Mutex};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

/// The real clock, used by the running daemon.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// A clock that only moves when told to. The daemon never constructs one;
/// it exists so callers driving the scheduler can do so deterministically.
#[derive(Debug, Clone)]
pub struct ManualClock {
    now: Arc<Mutex<DateTime<Utc>>>,
}

impl ManualClock {
    pub fn new(start: DateTime<Utc>) -> Self {
        Self {
            now: Arc::new(Mutex::new(start)),
        }
    }

    pub fn set(&self, to: DateTime<Utc>) {
        *self.now.lock().expect("clock poisoned") = to;
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.now.lock().expect("clock poisoned")
    }
}
