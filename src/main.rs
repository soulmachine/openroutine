//! `openroutine` — your AI agents' crontab, as markdown.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use openroutine::clock::{Clock, SystemClock};
use openroutine::config::{self, Config};
use openroutine::daemon::Daemon;
use std::path::PathBuf;
use std::time::Duration;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Where the Daemon keeps its own diagnostics, beside its state.
const DAEMON_LOG: &str = "daemon.log";

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
    /// Register a directory so its Tasks are scheduled.
    Add {
        /// The directory to watch.
        dir: PathBuf,
        /// A name for it; defaults to the directory's own.
        #[arg(long)]
        name: Option<String>,
    },
    /// Stop watching a directory.
    Remove { dir: PathBuf },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    // Diagnostics belong on stderr so `openroutine list | ...` stays a clean
    // stream of report. `serve` adds a file beside its state as well.
    let to_stderr = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_writer(std::io::stderr);

    let config_path = match cli.config.clone() {
        Some(path) => path,
        None => config::default_config_path()?,
    };

    // `serve` runs for days, so its diagnostics are also kept beside its
    // state. Only once the config says where that is — a daemon must never
    // drop a log file in whatever directory it happened to be started from.
    let daemon_log = matches!(cli.command, Command::Serve)
        .then(|| {
            Config::load(&config_path)
                .and_then(|config| config.state_dir())
                .ok()
        })
        .flatten()
        .and_then(|state_dir| {
            std::fs::create_dir_all(&state_dir).ok()?;
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(state_dir.join(DAEMON_LOG))
                .ok()?;
            Some(
                tracing_subscriber::fmt::layer()
                    .with_target(false)
                    .with_ansi(false)
                    .with_writer(std::sync::Arc::new(file)),
            )
        });
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let config_path = match cli.config {
        Some(path) => path,
        None => config::default_config_path()?,
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(to_stderr)
        .with(daemon_log)
        .init();

    match cli.command {
        Command::Serve => serve(&config_path).await,
        Command::List => list(&config_path),
        Command::Add { dir, name } => add(&config_path, &dir, name.as_deref()),
        Command::Remove { dir } => remove(&config_path, &dir),
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
    let mut report = openroutine::list::render(
        &scan.tasks,
        &state,
        SystemClock.now(),
        &openroutine::zone::host(),
    );

    // A Project we could not read is not an empty one, and must not look
    // like one.
    for project in &config.projects {
        let name = project.resolved_name();
        if !scan.reachable_projects.contains(&name) {
            report.push_str(&format!(
                "\nproject {name:?} is unreachable: cannot read {}\n",
                project.path.display()
            ));
        }
    }

    // `openroutine list | head` closes the pipe early; that's the reader's
    // choice, not an error worth a panic.
    match std::io::Write::write_all(&mut std::io::stdout().lock(), report.as_bytes()) {
        Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => Ok(()),
        other => other.map_err(Into::into),
    }
}

/// Registers a Project, editing the config in place.
fn add(config_path: &std::path::Path, dir: &std::path::Path, name: Option<&str>) -> Result<()> {
    let dir = dir
        .canonicalize()
        .with_context(|| format!("no such directory: {}", dir.display()))?;
    let config = Config::load(config_path)?;

    if let Some(existing) = config
        .projects
        .iter()
        .find(|project| project.path.canonicalize().is_ok_and(|path| path == dir))
    {
        println!(
            "{} is already registered as {:?}.",
            dir.display(),
            existing.resolved_name()
        );
        return Ok(());
    }

    let name = name.map(str::to_string).unwrap_or_else(|| basename(&dir));
    if let Some(clash) = config
        .projects
        .iter()
        .find(|project| project.resolved_name() == name)
    {
        anyhow::bail!(
            "a project called {name:?} is already registered ({}); \
             pick another with --name",
            clash.path.display()
        );
    }

    let mut document = read_document(config_path)?;
    let mut entry = toml_edit::Table::new();
    entry["path"] = toml_edit::value(dir.display().to_string());
    entry["name"] = toml_edit::value(name.clone());
    document["projects"]
        .or_insert(toml_edit::Item::ArrayOfTables(
            toml_edit::ArrayOfTables::new(),
        ))
        .as_array_of_tables_mut()
        .context("`projects` in the config is not a list of tables")?
        .push(entry);
    write_document(config_path, &document)?;

    println!("Registered {} as {name:?}.", dir.display());
    Ok(())
}

/// Unregisters a Project. Its Tasks stop being scheduled; nothing on disk in
/// the Project itself is touched.
fn remove(config_path: &std::path::Path, dir: &std::path::Path) -> Result<()> {
    let wanted = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut document = read_document(config_path)?;

    let Some(projects) = document
        .get_mut("projects")
        .and_then(|item| item.as_array_of_tables_mut())
    else {
        anyhow::bail!("{} is not registered", dir.display());
    };

    let before = projects.len();
    projects.retain(|entry| {
        let path = entry
            .get("path")
            .and_then(|path| path.as_str())
            .unwrap_or("");
        let path = std::path::Path::new(path);
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf()) != wanted
    });
    if projects.len() == before {
        anyhow::bail!("{} is not registered", dir.display());
    }

    write_document(config_path, &document)?;
    println!("Removed {}.", wanted.display());
    Ok(())
}

fn basename(dir: &std::path::Path) -> String {
    dir.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "project".to_string())
}

fn read_document(path: &std::path::Path) -> Result<toml_edit::DocumentMut> {
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("reading config at {}", path.display()))?;
    raw.parse::<toml_edit::DocumentMut>()
        .with_context(|| format!("parsing config at {}", path.display()))
}

fn write_document(path: &std::path::Path, document: &toml_edit::DocumentMut) -> Result<()> {
    // The config is hand-owned and irreplaceable: write beside it and rename,
    // so an interrupted write can never leave a truncated one behind.
    let temporary = path.with_extension("toml.tmp");
    std::fs::write(&temporary, document.to_string())
        .with_context(|| format!("writing config at {}", temporary.display()))?;
    std::fs::rename(&temporary, path)
        .with_context(|| format!("replacing config at {}", path.display()))
}

async fn serve(config_path: &std::path::Path) -> Result<()> {
    // Before anything slow: a scan of a large project takes time, and a
    // service manager that signals during startup must not have to kill us.
    let mut shutdown = Shutdown::listen()?;

    let config = Config::load(config_path)?;
    let mut daemon = Daemon::new(config, std::sync::Arc::new(SystemClock))?
        .watching_config(config_path.to_path_buf());

    std::fs::create_dir_all(daemon.state_dir())
        .with_context(|| format!("creating state dir {}", daemon.state_dir().display()))?;

    // Held for the whole run: a second scheduler over one state directory
    // would fire the same Tasks twice.
    let lock = openroutine::lock::DaemonLock::acquire(daemon.state_dir())?;
    tracing::debug!(lock = %lock.path().display(), "holding the daemon lock");

    // Safe only because the lock says no other Daemon is alive: any Run
    // still marked running belongs to a process that is gone.
    daemon.recover_interrupted_runs();
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
