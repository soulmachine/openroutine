//! OpenRoutine — the open-source alternative to Claude Code Routines and
//! ChatGPT scheduled tasks. One daemon: scheduler, API, and UI.
//!
//! Vocabulary follows `CONTEXT.md`: a **Task** is one `.cron.md` file in a
//! **Project**; a due **Tick** becomes exactly one **Run** or one **Skip**;
//! an **Agent** is a command template.

pub mod clock;
pub mod config;
pub mod daemon;
pub mod digest;
pub mod discovery;
pub mod jitter;
pub mod list;
pub mod run;
pub mod runner;
pub mod schedule;
pub mod state;
pub mod task;
pub mod zone;
