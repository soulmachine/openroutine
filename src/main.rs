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
    /// Register a Task file so it is scheduled.
    Add {
        /// The markdown file defining the Task.
        file: PathBuf,
    },
    /// Unregister a Task. The file is left alone unless you say otherwise.
    Remove {
        /// The Task's name, or the path of its file.
        task: String,
        /// Delete the file as well as unregistering it.
        #[arg(long)]
        delete: bool,
    },
    /// Re-read the config and every registered Task file.
    Reload {
        /// Report on one Task; the reload itself is always complete.
        task: Option<String>,
    },
    /// Write a starter config.
    Init,
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
    // `open` was this command's name through 0.2 and 1.0.0; kept as an alias
    // so the rename costs nobody their muscle memory or their scripts.
    #[command(alias = "open")]
    Dashboard,
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
        Command::Add { file } => add(&config_path, &file).await,
        Command::Remove { task, delete } => remove(&config_path, &task, delete).await,
        Command::Reload { task } => reload(&config_path, task.as_deref()).await,
        Command::Init => init(&config_path),
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
        Command::Dashboard => dashboard(&config_path),
        Command::Pause { task, all } => hold(&config_path, task.as_deref(), all, true).await,
        Command::Resume { task, all } => hold(&config_path, task.as_deref(), all, false).await,
    }
}

/// Reads Task files straight from disk — no Daemon required, and nothing
/// written back.
fn list(config_path: &std::path::Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let tasks = openroutine::registry::load_all(&config);
    // State is disposable by design; a damaged one must not stop a read.
    let state = openroutine::state::State::load(&config.state_dir()?).unwrap_or_else(|error| {
        tracing::warn!("ignoring unreadable run state: {error:#}");
        openroutine::state::State::default()
    });
    let report = openroutine::list::render(
        &tasks,
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

/// Registers one Task file, editing the config in place.
///
/// Validated here rather than at the next Tick: a file that cannot run should
/// be refused in front of whoever just wrote it.
async fn add(config_path: &std::path::Path, file: &std::path::Path) -> Result<()> {
    let file = file
        .canonicalize()
        .with_context(|| format!("no such file: {}", file.display()))?;
    if !file.is_file() {
        anyhow::bail!("{} is not a file", file.display());
    }
    let config = Config::load(config_path)?;

    if config.tasks.iter().any(|task| same_file(task, &file)) {
        println!("{} is already registered.", file.display());
        return Ok(());
    }

    let candidate = openroutine::registry::load_one(&file, &config);
    if let openroutine::registry::TaskHealth::Broken { error, .. } = &candidate.health {
        anyhow::bail!("{}: {error}", file.display());
    }
    // Uniqueness is decided on the id, not the written name: two different
    // names can derive the same id, and it is the id that becomes a run
    // directory and an API path.
    if let Some(clash) = openroutine::registry::load_all(&config)
        .iter()
        .find(|task| task.id == candidate.id)
    {
        anyhow::bail!(
            "the id {:?} is already registered ({}); \
             give one of them a different `name`",
            candidate.id,
            clash.path.display()
        );
    }

    let mut document = read_document(config_path)?;
    document
        .entry("tasks")
        .or_insert(toml_edit::Item::Value(toml_edit::Value::Array(
            toml_edit::Array::new(),
        )))
        .as_array_mut()
        .context("`tasks` in the config is not an array")?
        .push(file.display().to_string());
    write_document(config_path, &document)?;

    // The id is what every other command wants, and deriving it from the
    // name is lossy — so say what it came out as whenever it differs.
    match candidate.definition().map(|definition| &definition.name) {
        Some(name) if name != &candidate.id => println!(
            "Registered {:?} as {:?} ({}).",
            name,
            candidate.id,
            file.display()
        ),
        _ => println!("Registered {:?} ({}).", candidate.id, file.display()),
    }
    for warning in candidate.warnings() {
        println!("warning: {warning}");
    }
    nudge_daemon(config_path).await;
    Ok(())
}

/// Unregisters a Task, by name or by path.
///
/// The file is the user's, so it stays where it is unless `--delete` says
/// otherwise: `remove` undoes `add`, nothing more.
async fn remove(config_path: &std::path::Path, task: &str, delete: bool) -> Result<()> {
    let config = Config::load(config_path)?;
    let wanted = registered_path(&config, task)?;

    let mut document = read_document(config_path)?;
    let Some(tasks) = document
        .get_mut("tasks")
        .and_then(|item| item.as_array_mut())
    else {
        anyhow::bail!("{task} is not registered");
    };

    let before = tasks.len();
    tasks.retain(|entry| {
        entry
            .as_str()
            .is_none_or(|path| !same_file(std::path::Path::new(path), &wanted))
    });
    if tasks.len() == before {
        anyhow::bail!("{task} is not registered");
    }
    write_document(config_path, &document)?;
    println!("Unregistered {}.", wanted.display());

    if delete {
        match std::fs::remove_file(&wanted) {
            Ok(()) => println!("Deleted {}.", wanted.display()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                println!("{} was already gone.", wanted.display())
            }
            Err(error) => {
                return Err(error).with_context(|| format!("deleting {}", wanted.display()));
            }
        }
    }

    nudge_daemon(config_path).await;
    Ok(())
}

/// The registered file a name or path refers to.
///
/// A path always works, which is what makes a file too broken to state its
/// own name still removable.
fn registered_path(config: &Config, task: &str) -> Result<PathBuf> {
    let literal = std::path::Path::new(task);
    if let Some(path) = config
        .tasks
        .iter()
        .find(|candidate| same_file(candidate, literal))
    {
        return Ok(path.clone());
    }

    let loaded = openroutine::registry::load_all(config);
    loaded
        .iter()
        .find(|candidate| candidate.id == task)
        .map(|candidate| candidate.path.clone())
        .with_context(|| format!("no registered task called {task:?}"))
}

/// Whether two paths name the same file, canonicalising what exists and
/// comparing literally what does not.
fn same_file(left: &std::path::Path, right: &std::path::Path) -> bool {
    let resolve =
        |path: &std::path::Path| path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    resolve(left) == resolve(right)
}

/// Asks a running Daemon to Reload, if there is one.
///
/// Best-effort by design: nothing is watched, so a Daemon that isn't running
/// simply picks the change up when it next starts.
async fn nudge_daemon(config_path: &std::path::Path) {
    let config = match Config::load(config_path) {
        Ok(config) => config,
        Err(_) => return,
    };
    let Ok(state_dir) = config.state_dir() else {
        return;
    };
    if !openroutine::lock::is_held(&state_dir) {
        println!("The daemon is not running; it will pick this up when it starts.");
        return;
    }
    match call_api(config_path, "POST", "/v1/reload", None).await {
        Ok(_) => println!("Reloaded the running daemon."),
        Err(error) => println!("Could not reload the running daemon: {error:#}"),
    }
}

/// Re-reads everything, then says what that found.
async fn reload(config_path: &std::path::Path, only: Option<&str>) -> Result<()> {
    let report = call_api(config_path, "POST", "/v1/reload", None).await?;

    if let Some(error) = report.get("configError").and_then(|value| value.as_str()) {
        println!("config: {error}");
        println!("The daemon kept the config it already had.");
    }

    let empty = Vec::new();
    let tasks = report
        .get("tasks")
        .and_then(|value| value.as_array())
        .unwrap_or(&empty);

    let mut shown = 0;
    for task in tasks {
        let name = task.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        if only.is_some_and(|wanted| wanted != name) {
            continue;
        }
        shown += 1;
        let status = task.get("status").and_then(|v| v.as_str()).unwrap_or("?");
        println!("{status:<9} {name}");
        for key in ["error", "novelty"] {
            if let Some(note) = task.get(key).and_then(|v| v.as_str()) {
                println!("          {note}");
            }
        }
        for warning in task
            .get("warnings")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty)
        {
            if let Some(warning) = warning.as_str() {
                println!("          warning: {warning}");
            }
        }
    }

    match only {
        Some(wanted) if shown == 0 => {
            println!(
                "Reloaded {} task(s); none is called {wanted:?}.",
                tasks.len()
            )
        }
        _ => println!(
            "\nReloaded {} task{}.",
            tasks.len(),
            if tasks.len() == 1 { "" } else { "s" }
        ),
    }
    Ok(())
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

/// Writes the config, and prints a Task to copy, so the first five minutes
/// need no documentation.
///
/// Nothing is registered here and no file is written anywhere else:
/// registering is `add`, and it is the user's move to make.
fn init(config_path: &std::path::Path) -> Result<()> {
    if config_path.exists() {
        anyhow::bail!(
            "{} already exists; edit it, or use `add` to register a task file",
            config_path.display()
        );
    }

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(config_path, STARTER_CONFIG)
        .with_context(|| format!("writing {}", config_path.display()))?;

    println!("Wrote config {}", config_path.display());
    println!("\nA task is one markdown file. Save this as hello.md:\n");
    println!("{SAMPLE_TASK}");
    println!("Then:  openroutine add hello.md          # register it");
    println!("       openroutine list                  # what would run");
    println!("       openroutine run hello --dry-run   # what it would do");
    println!("       openroutine serve                 # start the scheduler");
    println!("\nNothing fires until a daemon is running. `openroutine install`");
    println!("registers one with your service manager, without sudo.");
    Ok(())
}

const STARTER_CONFIG: &str = "# openroutine — https://openroutine.dev\n\
     \n\
     # The agent a task gets when it names none.\n\
     default_agent = \"claude\"\n\
     \n\
     # Every registered task file, by absolute path. `openroutine add` writes\n\
     # these; editing them by hand is fine too.\n\
     tasks = []\n\
     \n\
     # An agent is a command template. {prompt} is substituted as a\n\
     # single argument; without it, the prompt arrives on stdin.\n\
     [agents.claude]\n\
     cmd = \"claude --dangerously-skip-permissions --effort xhigh -p {prompt}\"\n\
     \n\
     [agents.codex]\n\
     cmd = \"codex --yolo -c model_reasoning_effort=xhigh exec {prompt}\"\n";

const SAMPLE_TASK: &str = "---\n\
     name: hello\n\
     description: Say hello, nightly\n\
     cron: \"0 9 * * *\"\n\
     ---\n\
     \n\
     Say hello, then stop. This is a sample task — edit or delete it.";

/// Summarises the daemon and what it would be running.
fn status(config_path: &std::path::Path) -> Result<()> {
    let config = Config::load(config_path)?;
    let state_dir = config.state_dir()?;
    let tasks = openroutine::registry::load_all(&config);
    let state = openroutine::state::State::load(&state_dir).unwrap_or_default();

    let broken = tasks.iter().filter(|task| task.is_broken()).count();
    let flagged = tasks
        .iter()
        .filter(|task| {
            task.novelty(state.last_run_digest(&task.id))
                .note()
                .is_some()
        })
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
    println!("files:    {} registered", config.tasks.len());
    let disabled = tasks.iter().filter(|task| task.is_disabled()).count();
    println!(
        "tasks:    {} ready, {disabled} disabled, {broken} broken, {flagged} flagged",
        tasks.len() - broken - disabled
    );
    if state.paused {
        println!("paused:   yes — nothing will fire until you resume");
    } else {
        let held: Vec<&str> = tasks
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
    let tasks = openroutine::registry::load_all(&config);
    let task = resolve(&tasks, wanted)?;

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
    let tasks = openroutine::registry::load_all(&config);
    let task = resolve(&tasks, wanted)?;

    if !dry_run {
        let id = task.id.clone();
        let body = text.map(|text| serde_json::json!({ "text": text }).to_string());
        let answer = call_api(
            config_path,
            "POST",
            &format!("/v1/tasks/{id}/fire"),
            body.as_deref(),
        )
        .await
        .map_err(|error| stale_daemon_hint(error, &id))?;
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

/// Explains a Task the Daemon has never heard of but disk clearly has.
///
/// Definitions reach the Daemon only at a Reload, so "no such task" from a
/// running Daemon about a Task that is plainly registered means one thing.
fn stale_daemon_hint(error: anyhow::Error, id: &str) -> anyhow::Error {
    if format!("{error:#}").contains("no task called") {
        return error.context(format!(
            "{id:?} is registered but the running daemon has not taken it in; \
             run `openroutine reload`"
        ));
    }
    error
}

/// Finds the Task someone meant: its name, or the path of its file.
///
/// Names are unique, so this is an exact match rather than a search. The path
/// form is what reaches a file too broken to state a name of its own.
fn resolve<'a>(
    tasks: &'a [openroutine::registry::RegisteredTask],
    wanted: &str,
) -> Result<&'a openroutine::registry::RegisteredTask> {
    // An id belongs to the file that claimed it first; a later file deriving
    // the same one is Broken and answers only to its path. So prefer the
    // holder, and fall back to a Broken match when that is all there is.
    let by_id = |id: &str| {
        tasks
            .iter()
            .find(|task| task.id == id && !task.is_broken())
            .or_else(|| tasks.iter().find(|task| task.id == id))
    };
    if let Some(exact) = by_id(wanted) {
        return Ok(exact);
    }
    // Typing the written name works too, put through the same derivation the
    // file's own name went through — so `run "Nightly triage"` finds
    // `nightly-triage` without the user having to slug it by hand.
    if let Ok(derived) = openroutine::task::slugify(wanted)
        && let Some(exact) = by_id(&derived)
    {
        return Ok(exact);
    }

    let literal = std::path::Path::new(wanted);
    tasks
        .iter()
        .find(|task| same_file(&task.path, literal))
        .with_context(|| format!("no task called {wanted:?}"))
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
    let mut resolved = None;
    let path = match task {
        Some(name) => {
            let config = Config::load(config_path)?;
            let tasks = openroutine::registry::load_all(&config);
            let id = resolve(&tasks, name)?.id.clone();
            let path = format!("/v1/tasks/{id}/{}", verb(paused));
            resolved = Some(id);
            path
        }
        None => format!("/v1/{}", verb(paused)),
    };

    let answer =
        call_api(config_path, "POST", &path, None)
            .await
            .map_err(|error| match &resolved {
                Some(id) => stale_daemon_hint(error, id),
                None => error,
            })?;
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
fn dashboard(config_path: &std::path::Path) -> Result<()> {
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

    // Definitions were read once, at startup. Nothing re-reads them on a
    // timer or a file event: a Reload is asked for, through the API, and
    // until one arrives the Daemon runs exactly what it was told to.
    let mut ticker = tokio::time::interval(TICK_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if let Err(error) = daemon.lock().await.tick().await {
                    tracing::error!("tick failed: {error:#}");
                }
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
