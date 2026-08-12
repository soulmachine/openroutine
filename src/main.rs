//! `openroutine` — your AI agents' crontab, as markdown.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use openroutine::clock::{Clock, SystemClock};
use openroutine::config::{self, Config};
use openroutine::daemon::Daemon;
use std::path::PathBuf;
use std::time::Duration;

/// How often the scheduler looks for due Ticks. Minute-resolution cron needs
/// nothing finer; the cost of a pass is a comparison per Task.
const TICK_INTERVAL: Duration = Duration::from_secs(1);
/// How often the Projects are rescanned regardless of what the watcher says.
/// The watcher makes reloads prompt; this is what makes them certain.
const RESCAN_INTERVAL: Duration = Duration::from_secs(30);
/// How long to let a burst of file events settle before rescanning, so a
/// branch checkout produces one reload rather than fifty.
const SETTLE: Duration = Duration::from_millis(200);

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
    /// Show every Task, its schedule, and its health.
    List,
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
        Command::List => list(&config_path),
    }
}

/// Reads Task files straight from disk — no Daemon required, and nothing
/// written back.
fn list(config_path: &std::path::Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let scan = openroutine::discovery::scan_all(&config);
    // State is disposable by design; a damaged one must not stop a read.
    let state = openroutine::state::State::load(&config.state_dir()?).unwrap_or_else(|error| {
        tracing::warn!("ignoring unreadable run state: {error:#}");
        openroutine::state::State::default()
    });
    let report = openroutine::list::render(
        &scan.tasks,
        &state,
        SystemClock.now(),
        &openroutine::zone::host(),
    );

    // `openroutine list | head` closes the pipe early; that's the reader's
    // choice, not an error worth a panic.
    match std::io::Write::write_all(&mut std::io::stdout().lock(), report.as_bytes()) {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other.map_err(Into::into),
    }
}

async fn serve(config_path: &std::path::Path) -> Result<()> {
    // Before anything slow: a scan of a large project takes time, and a
    // service manager that signals during startup must not have to kill us.
    let mut shutdown = Shutdown::listen()?;

    let config = Config::load(config_path)?;
    let mut daemon = Daemon::new(config, std::sync::Arc::new(SystemClock))?;

    std::fs::create_dir_all(daemon.state_dir())
        .with_context(|| format!("creating state dir {}", daemon.state_dir().display()))?;

    daemon.reload().await?;
    tracing::info!(
        state_dir = %daemon.state_dir().display(),
        tasks = daemon.task_count(),
        broken = daemon.broken_count(),
        "openroutine serving"
    );

    let mut ticker = tokio::time::interval(TICK_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut rescan = tokio::time::interval(RESCAN_INTERVAL);
    rescan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    rescan.tick().await; // the first tick is immediate; we just scanned

    let mut changes = watch_projects(daemon.project_dirs());

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(error) = daemon.tick().await {
                    tracing::error!("tick failed: {error:#}");
                }
            }
            _ = rescan.tick() => reload(&mut daemon).await,
            Some(()) = changes.recv() => {
                // Let the rest of the burst arrive, then take everything at
                // once — and drain, so a checkout is one reload, not fifty.
                tokio::time::sleep(SETTLE).await;
                while changes.try_recv().is_ok() {}
                reload(&mut daemon).await;
            }
            signal = shutdown.recv() => {
                tracing::info!(signal, in_flight = daemon.running_count(), "shutting down");
                return Ok(());
            }
        }
    }
}

/// Rescans, keeping the Daemon alive if a scan fails — a bad moment on disk
/// should not take the scheduler down with it.
async fn reload(daemon: &mut Daemon) {
    match daemon.reload().await {
        Ok(()) => tracing::debug!(tasks = daemon.task_count(), "rescanned"),
        Err(error) => tracing::error!("rescan failed: {error:#}"),
    }
}

/// Watches every Project for changes.
///
/// Events only ever *hasten* a rescan: the periodic scan is the source of
/// truth, so a missed or coalesced event costs latency, never correctness.
fn watch_projects(dirs: Vec<PathBuf>) -> tokio::sync::mpsc::Receiver<()> {
    let (sender, receiver) = tokio::sync::mpsc::channel(1);

    std::thread::spawn(move || {
        use notify::Watcher;

        let notify_sender = sender.clone();
        let mut watcher = match notify::recommended_watcher(move |event| {
            if matches!(event, Ok(notify::Event { .. })) {
                // A full channel already means "rescan pending".
                let _ = notify_sender.try_send(());
            }
        }) {
            Ok(watcher) => watcher,
            Err(error) => {
                tracing::warn!(
                    "file watching unavailable, falling back to periodic rescans: {error}"
                );
                return;
            }
        };

        for dir in &dirs {
            if let Err(error) = watcher.watch(dir, notify::RecursiveMode::Recursive) {
                tracing::warn!(dir = %dir.display(), "cannot watch for changes: {error}");
            }
        }

        // The watcher stops the moment it is dropped, so hold it until the
        // daemon lets go of the channel.
        while !sender.is_closed() {
            std::thread::sleep(Duration::from_millis(250));
        }
    });

    receiver
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
