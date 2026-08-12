//! `openroutine` — your AI agents' crontab, as markdown.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use openroutine::clock::{Clock, SystemClock};
use openroutine::config::{self, Config};
use openroutine::daemon::Daemon;
use std::future::IntoFuture;
use std::path::PathBuf;
use std::time::Duration;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// Where the Daemon keeps its own diagnostics, beside its state.
const DAEMON_LOG: &str = "daemon.log";
/// How long a stopping daemon waits for Runs already in flight.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);
/// Set this to start with everything held.
const DISABLE_ENV: &str = "OPENROUTINE_DISABLE";

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
    /// Write a starter config and a sample Task.
    Init {
        /// The directory to register and put the sample in.
        #[arg(default_value = ".")]
        dir: PathBuf,
    },
    /// Summarise the daemon and what it would be running.
    Status,
    /// Show a Task's most recent Run.
    Logs {
        task: String,
        /// Keep printing as the Run writes more.
        #[arg(long)]
        follow: bool,
    },
    /// Register the daemon with your service manager, without sudo.
    Install {
        /// Show what would be written instead of writing it.
        #[arg(long)]
        print: bool,
    },
    /// Unregister the daemon. Config, state, and Tasks are left alone.
    Uninstall,
    /// Hold a Task, or everything.
    Pause {
        /// Which Task; omit with --all.
        task: Option<String>,
        /// Hold every Task on this machine.
        #[arg(long)]
        all: bool,
    },
    /// Release a Task, or everything.
    Resume {
        task: Option<String>,
        #[arg(long)]
        all: bool,
    },
    /// Open the local web UI in a browser.
    Open,
    /// Print the API token, or replace it.
    Token {
        /// Replace the token. Anything using the old one stops working.
        #[arg(long)]
        rotate: bool,
    },
    /// Fire a Task now, or describe what firing it would do.
    Run {
        task: String,
        /// Describe the Run instead of starting one.
        #[arg(long)]
        dry_run: bool,
        /// Context for this run: information for the agent, never
        /// instructions, and it cannot redefine the task.
        #[arg(long)]
        text: Option<String>,
    },
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
        Command::Init { dir } => init(&config_path, &dir),
        Command::Status => status(&config_path),
        Command::Logs { task, follow } => logs(&config_path, &task, follow),
        Command::Run {
            task,
            dry_run,
            text,
        } => run(&config_path, &task, dry_run, text.as_deref()).await,
        Command::Install { print } => install(&config_path, print),
        Command::Uninstall => uninstall(&config_path),
        Command::Token { rotate } => token(&config_path, rotate),
        Command::Open => open_ui(&config_path),
        Command::Pause { task, all } => hold(&config_path, task.as_deref(), all, true).await,
        Command::Resume { task, all } => hold(&config_path, task.as_deref(), all, false).await,
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

/// Writes a config and a sample Task, so the first five minutes need no
/// documentation.
fn init(config_path: &std::path::Path, dir: &std::path::Path) -> Result<()> {
    if config_path.exists() {
        anyhow::bail!(
            "{} already exists; edit it, or use `add` to register another directory",
            config_path.display()
        );
    }
    let dir = dir
        .canonicalize()
        .with_context(|| format!("no such directory: {}", dir.display()))?;

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(config_path, starter_config(&dir))
        .with_context(|| format!("writing {}", config_path.display()))?;

    let sample = dir.join("hello.cron.md");
    if !sample.exists() {
        std::fs::write(&sample, SAMPLE_TASK)
            .with_context(|| format!("writing {}", sample.display()))?;
    }

    println!("Wrote config {}", config_path.display());
    println!("Wrote sample task {}", sample.display());
    println!("\nTry:  openroutine list");
    println!("      openroutine run hello --dry-run");
    Ok(())
}

fn starter_config(dir: &std::path::Path) -> String {
    format!(
        "# openroutine — https://openroutine.dev\n\
         \n\
         # The agent a task gets when it names none.\n\
         default_agent = \"claude\"\n\
         \n\
         # Directories whose *.cron.md files should be scheduled.\n\
         [[projects]]\n\
         path = {dir:?}\n\
         \n\
         # An agent is a command template. {{prompt}} is substituted as a\n\
         # single argument; without it, the prompt arrives on stdin.\n\
         [agents.claude]\n\
         cmd = \"claude -p {{prompt}}\"\n\
         \n\
         [agents.codex]\n\
         cmd = \"codex exec {{prompt}}\"\n",
        dir = dir.display().to_string(),
    )
}

const SAMPLE_TASK: &str = "---\n\
     description: Say hello, nightly\n\
     cron: \"0 9 * * *\"\n\
     ---\n\
     \n\
     Say hello, then stop. This is a sample task — edit or delete it.\n";

/// Summarises the daemon and what it would be running.
fn status(config_path: &std::path::Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let state_dir = config.state_dir()?;
    let scan = openroutine::discovery::scan_all(&config);
    let state = openroutine::state::State::load(&state_dir).unwrap_or_default();

    let broken = scan.tasks.iter().filter(|task| task.is_broken()).count();
    let flagged = scan
        .tasks
        .iter()
        .filter(|task| {
            task.novelty(state.last_run_digest(&task.id))
                .note()
                .is_some()
        })
        .count();
    let unreachable = config
        .projects
        .iter()
        .filter(|project| !scan.reachable_projects.contains(&project.resolved_name()))
        .count();

    let daemon = if openroutine::lock::is_held(&state_dir) {
        match openroutine::lock::holder(&state_dir) {
            Some(pid) => format!("running (pid {pid})"),
            None => "running".to_string(),
        }
    } else {
        "not running".to_string()
    };

    println!("daemon:   {daemon}");
    println!("config:   {}", config_path.display());
    println!("state:    {}", state_dir.display());
    println!(
        "projects: {}{}",
        config.projects.len(),
        if unreachable > 0 {
            format!(" ({unreachable} unreachable)")
        } else {
            String::new()
        }
    );
    let disabled = scan.tasks.iter().filter(|task| task.is_disabled()).count();
    println!(
        "tasks:    {} ready, {disabled} disabled, {broken} broken, {flagged} flagged",
        scan.tasks.len() - broken - disabled
    );
    if state.paused {
        println!("paused:   yes — nothing will fire until you resume");
    } else {
        let held: Vec<&str> = scan
            .tasks
            .iter()
            .filter(|task| state.is_paused(&task.id))
            .map(|task| task.id.as_str())
            .collect();
        println!(
            "paused:   {}",
            if held.is_empty() {
                "no".to_string()
            } else {
                format!("{} task(s): {}", held.len(), held.join(", "))
            }
        );
    }
    Ok(())
}

/// Prints a Task's most recent Run, straight from disk.
fn logs(config_path: &std::path::Path, wanted: &str, follow: bool) -> Result<()> {
    let config = Config::load(config_path)?;
    let scan = openroutine::discovery::scan_all(&config);
    let task = resolve(&scan.tasks, wanted)?;

    let runs = openroutine::run::task_runs_dir(&config.state_dir()?, &task.id);
    let mut directories: Vec<PathBuf> = std::fs::read_dir(&runs)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    directories.sort();

    let Some(latest) = directories.last() else {
        println!("no runs yet for {}", task.id);
        return Ok(());
    };

    let log = latest.join(openroutine::run::RUN_LOG);
    if follow {
        follow_file(&log)
    } else {
        let contents =
            std::fs::read_to_string(&log).with_context(|| format!("reading {}", log.display()))?;
        print!("{contents}");
        Ok(())
    }
}

/// Prints a file and keeps printing as it grows.
fn follow_file(path: &std::path::Path) -> Result<()> {
    use std::io::{Read, Seek, Write};

    let mut file =
        std::fs::File::open(path).with_context(|| format!("reading {}", path.display()))?;
    let mut buffer = vec![0u8; 8192];
    let mut stdout = std::io::stdout();

    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            std::thread::sleep(Duration::from_millis(200));
            let length = file.metadata()?.len();
            if length < file.stream_position()? {
                // Truncated or replaced under us; start again from the top.
                file.seek(std::io::SeekFrom::Start(0))?;
            }
            continue;
        }
        match stdout.write_all(&buffer[..read]) {
            Ok(()) => stdout.flush().ok(),
            Err(error) if error.kind() == std::io::ErrorKind::BrokenPipe => return Ok(()),
            Err(error) => return Err(error.into()),
        };
    }
}

/// Describes what a Run would do, or explains why it cannot.
async fn run(
    config_path: &std::path::Path,
    wanted: &str,
    dry_run: bool,
    text: Option<&str>,
) -> Result<()> {
    let config = Config::load(config_path)?;
    let scan = openroutine::discovery::scan_all(&config);
    let task = resolve(&scan.tasks, wanted)?;

    if !dry_run {
        let id = task.id.clone();
        let body = text.map(|text| serde_json::json!({ "text": text }).to_string());
        let answer = call_api(
            config_path,
            "POST",
            &format!("/v1/tasks/{id}/fire"),
            body.as_deref(),
        )
        .await?;
        println!(
            "Started {} — {}",
            answer["run_id"].as_str().unwrap_or("a run"),
            answer["log"].as_str().unwrap_or("see `openroutine logs`")
        );
        return Ok(());
    }

    let plan =
        openroutine::plan::render(task, &config, SystemClock.now(), &openroutine::zone::host())
            .map_err(|reason| anyhow::anyhow!("{}: {reason}", task.id))?;
    print!("{plan}");
    Ok(())
}

/// Finds the Task someone meant: a full `<project>/<name>` id, or a bare
/// name where only one Task answers to it.
fn resolve<'a>(
    tasks: &'a [openroutine::discovery::ScannedTask],
    wanted: &str,
) -> Result<&'a openroutine::discovery::ScannedTask> {
    if let Some(exact) = tasks.iter().find(|task| task.id == wanted) {
        return Ok(exact);
    }

    let matches: Vec<&openroutine::discovery::ScannedTask> = tasks
        .iter()
        .filter(|task| {
            task.id
                .split_once('/')
                .is_some_and(|(_, name)| name == wanted)
        })
        .collect();

    match matches.as_slice() {
        [only] => Ok(only),
        [] => anyhow::bail!("no task called {wanted:?}"),
        several => anyhow::bail!(
            "{wanted:?} is ambiguous; say which one:\n{}",
            several
                .iter()
                .map(|task| format!("  {}", task.id))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    }
}

/// Holds or releases Tasks, through the daemon so a running one obeys.
async fn hold(
    config_path: &std::path::Path,
    task: Option<&str>,
    all: bool,
    paused: bool,
) -> Result<()> {
    if task.is_none() && !all {
        anyhow::bail!("name a task, or pass --all");
    }
    let path = match task {
        Some(name) => {
            let config = Config::load(config_path)?;
            let scan = openroutine::discovery::scan_all(&config);
            let id = resolve(&scan.tasks, name)?.id.clone();
            format!("/v1/tasks/{id}/{}", verb(paused))
        }
        None => format!("/v1/{}", verb(paused)),
    };

    let answer = call_api(config_path, "POST", &path, None).await?;
    println!(
        "{} {}",
        if paused { "Paused" } else { "Resumed" },
        answer["scope"].as_str().unwrap_or("everything")
    );
    Ok(())
}

fn verb(paused: bool) -> &'static str {
    if paused { "pause" } else { "resume" }
}

/// Asks the running daemon to do something.
async fn call_api(
    config_path: &std::path::Path,
    method: &str,
    path: &str,
    body: Option<&str>,
) -> Result<serde_json::Value> {
    let config = Config::load(config_path)?;
    let state_dir = config.state_dir()?;
    if !openroutine::lock::is_held(&state_dir) {
        anyhow::bail!("no daemon is running; start one with `openroutine serve`");
    }
    let token = openroutine::token::read(&state_dir)?
        .context("the daemon has no API token yet; start it once with `openroutine serve`")?;

    let answer = openroutine::client::send(&config.bind(), &token, method, path, body).await?;
    let parsed = answer.json();

    if answer.ok() {
        Ok(parsed)
    } else {
        anyhow::bail!(
            "{}",
            parsed["error"]["message"].as_str().unwrap_or(&answer.body)
        )
    }
}

/// Opens the UI, signed in.
///
/// The token rides in the URL once and is exchanged for a session cookie
/// straight away, so the long-lived secret does not end up in browser
/// storage — the same shape Jupyter uses, for the same reason.
fn open_ui(config_path: &std::path::Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let state_dir = config.state_dir()?;
    if !openroutine::lock::is_held(&state_dir) {
        anyhow::bail!("no daemon is running; start one with `openroutine serve`");
    }
    let token = openroutine::token::read(&state_dir)?
        .context("the daemon has no API token yet; start it once with `openroutine serve`")?;
    let url = format!("http://{}/?token={token}", config.bind());

    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(opener).arg(&url).status() {
        Ok(status) if status.success() => println!("Opened {}", config.bind()),
        _ => println!("Open this once, and it will remember you:\n{url}"),
    }
    Ok(())
}

/// Shows the API token, or replaces it.
fn token(config_path: &std::path::Path, rotate: bool) -> Result<()> {
    let state_dir = Config::load(config_path)?.state_dir()?;
    let token = if rotate {
        openroutine::token::create(&state_dir)?
    } else {
        openroutine::token::load_or_create(&state_dir)?
    };
    println!("{token}");
    Ok(())
}

/// Registers the daemon so it comes back by itself.
fn install(config_path: &std::path::Path, print: bool) -> Result<()> {
    let definition = service_definition(config_path)?;

    if print {
        println!("# {}", definition.path.display());
        print!("{}", definition.contents);
        for command in definition.activate.iter().chain(&definition.deactivate) {
            println!("# would run: {}", command.join(" "));
        }
        return Ok(());
    }

    openroutine::service::install(&definition)?;
    println!("Installed {}", definition.path.display());
    println!(
        "The daemon will start {} and be restarted if it stops.",
        if cfg!(target_os = "macos") {
            "when you log in (pair with auto-login on a headless machine)"
        } else {
            "at boot, with no login session needed"
        }
    );
    println!("Check it with:  openroutine status");
    Ok(())
}

fn uninstall(config_path: &std::path::Path) -> Result<()> {
    let definition = service_definition(config_path)?;
    if openroutine::service::uninstall(&definition)? {
        println!("Removed {}", definition.path.display());
    } else {
        println!(
            "Nothing to remove; {} was not there",
            definition.path.display()
        );
    }
    println!("Your config, state, and tasks are untouched.");
    Ok(())
}

fn service_definition(
    config_path: &std::path::Path,
) -> Result<openroutine::service::ServiceDefinition> {
    let binary = std::env::current_exe().context("finding this binary")?;
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")?;
    let config = config_path
        .canonicalize()
        .unwrap_or_else(|_| config_path.to_path_buf());
    openroutine::service::definition(&binary, &config, &home)
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

    // A way to start held that does not need the API to be reachable — for
    // a machine you want up and inspectable but firing nothing.
    if std::env::var_os(DISABLE_ENV).is_some_and(|value| !value.is_empty()) {
        daemon.set_paused(None, true)?;
        tracing::warn!("{DISABLE_ENV} is set: everything is paused and nothing will fire");
    }

    daemon.reload().await?;
    tracing::info!(
        state_dir = %daemon.state_dir().display(),
        tasks = daemon.task_count(),
        broken = daemon.broken_count(),
        "openroutine serving"
    );

    let state_dir = daemon.state_dir().clone();
    let bind = config_bind(config_path);
    let project_dirs = daemon.project_dirs();
    let daemon = std::sync::Arc::new(tokio::sync::Mutex::new(daemon));
    let api_token = openroutine::token::load_or_create(&state_dir)?;
    let listener = tokio::net::TcpListener::bind(&bind)
        .await
        .with_context(|| format!("listening on {bind}"))?;
    if !bind.starts_with("127.0.0.1:")
        && !bind.starts_with("localhost:")
        && !bind.starts_with("[::1]:")
    {
        tracing::warn!(
            %bind,
            "the api is reachable beyond this machine; it is guarded only by a bearer token \
             over plain http, so an ssh tunnel or a private network is the safer arrangement"
        );
    }
    tracing::info!(%bind, "api listening");
    let server = tokio::spawn(
        axum::serve(listener, {
            let api = openroutine::api::Api {
                daemon: std::sync::Arc::clone(&daemon),
                token: api_token,
            };
            openroutine::api::router(api.clone()).merge(openroutine::ui::router(api))
        })
        .into_future(),
    );

    let mut ticker = tokio::time::interval(TICK_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut rescan = tokio::time::interval(RESCAN_INTERVAL);
    rescan.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    rescan.tick().await; // the first tick is immediate; we just scanned

    let mut changes = watch_projects(project_dirs);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(error) = daemon.lock().await.tick().await {
                    tracing::error!("tick failed: {error:#}");
                }
            }
            _ = rescan.tick() => reload(&mut *daemon.lock().await).await,
            Some(()) = changes.recv() => {
                // Let the rest of the burst arrive, then take everything at
                // once — and drain, so a checkout is one reload, not fifty.
                tokio::time::sleep(SETTLE).await;
                while changes.try_recv().is_ok() {}
                reload(&mut *daemon.lock().await).await;
            }
            signal = shutdown.recv() => {
                server.abort();
                let in_flight = daemon.lock().await.running_count();
                tracing::info!(signal, in_flight, "shutting down");

                // Give whatever is mid-run a moment to finish and record
                // itself, rather than leaving it to be found and marked
                // interrupted next time. Bounded, because a run may have
                // asked for no timeout at all.
                if in_flight > 0 {
                    let waited = tokio::time::timeout(
                        SHUTDOWN_GRACE,
                        daemon.lock().await.wait_for_running(),
                    )
                    .await;
                    if waited.is_err() {
                        tracing::warn!(
                            "still running after {SHUTDOWN_GRACE:?}; they will be recorded as \
                             interrupted on the next start"
                        );
                    }
                }
                return Ok(());
            }
        }
    }
}

/// Where the API should listen, according to the config.
fn config_bind(config_path: &std::path::Path) -> String {
    Config::load(config_path)
        .map(|config| config.bind())
        .unwrap_or_else(|_| format!("127.0.0.1:{}", openroutine::api::DEFAULT_PORT))
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
