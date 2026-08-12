//! `openroutine` — your AI agents' crontab, as markdown.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use openroutine::clock::SystemClock;
use openroutine::config::{self, Config};
use openroutine::daemon::Daemon;
use std::path::PathBuf;
use std::time::Duration;

/// How often the scheduler looks for due Ticks. Minute-resolution cron needs
/// nothing finer; the cost of a pass is a comparison per Task.
const TICK_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Debug, Parser)]
#[command(
    name = "openroutine",
    version,
    about = "Your AI agents' crontab, as markdown"
)]
struct Cli {
    /// Config file to use (defaults to the XDG config path).
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the scheduler in the foreground.
    Serve,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();
    let config_path = match cli.config {
        Some(path) => path,
        None => config::default_config_path()?,
    };

    match cli.command {
        Command::Serve => serve(&config_path).await,
    }
}

async fn serve(config_path: &std::path::Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let mut daemon = Daemon::new(config, std::sync::Arc::new(SystemClock))?;

    std::fs::create_dir_all(daemon.state_dir())
        .with_context(|| format!("creating state dir {}", daemon.state_dir().display()))?;

    daemon.reload().await?;
    tracing::info!(
        state_dir = %daemon.state_dir().display(),
        tasks = daemon.task_count(),
        "openroutine serving"
    );

    let mut ticker = tokio::time::interval(TICK_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    // Registered once, before the loop: tokio installs a process-wide handler
    // on first use, so a stream recreated each iteration can miss a signal
    // that lands between iterations — and a daemon that ignores SIGTERM is a
    // daemon its service manager has to kill.
    let mut shutdown = Shutdown::listen()?;

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(error) = daemon.tick().await {
                    tracing::error!("tick failed: {error:#}");
                }
            }
            signal = shutdown.recv() => {
                tracing::info!(signal, in_flight = daemon.running_count(), "shutting down");
                return Ok(());
            }
        }
    }
}

/// SIGTERM and SIGINT, held open for the daemon's whole life.
struct Shutdown {
    #[cfg(unix)]
    terminate: tokio::signal::unix::Signal,
    #[cfg(unix)]
    interrupt: tokio::signal::unix::Signal,
}

impl Shutdown {
    fn listen() -> Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                terminate: signal(SignalKind::terminate()).context("listening for SIGTERM")?,
                interrupt: signal(SignalKind::interrupt()).context("listening for SIGINT")?,
            })
        }

        #[cfg(not(unix))]
        Ok(Self {})
    }

    /// Resolves with the signal's name once either arrives.
    async fn recv(&mut self) -> &'static str {
        #[cfg(unix)]
        {
            tokio::select! {
                _ = self.terminate.recv() => "SIGTERM",
                _ = self.interrupt.recv() => "SIGINT",
            }
        }

        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
            "ctrl-c"
        }
    }
}
