//! Daemon configuration: which Projects to watch, and what an Agent is.
//!
//! Human-owned and hand-editable. Which directories to watch defines
//! behaviour, so it lives here rather than in machine state.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// An unattended Agent that hangs is the worst failure mode, so Runs are
/// bounded unless the author says otherwise.
const DEFAULT_TIMEOUT: chrono::Duration = chrono::Duration::hours(1);
/// Enough history to explain recent behaviour; not enough to fill a disk.
const DEFAULT_MAX_RUNS_PER_TASK: usize = 50;
/// Enough to debug a Run; not enough to fill a disk.
const DEFAULT_MAX_LOG_BYTES: u64 = 100 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Where machine-owned state lives. Not a config-file key: storage
    /// locations are fixed by design (ADR-0001), so this is resolved from the
    /// XDG state directory and only set directly by callers that need to.
    #[serde(skip)]
    pub state_dir: Option<PathBuf>,
    #[serde(default)]
    pub projects: Vec<ProjectConfig>,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentConfig>,
    /// Environment handed to every Run, layered over the login shell's own
    /// and under whatever the Task itself sets.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// How many Runs may be in flight at once, across every Task. Unset
    /// means whatever the hardware tolerates.
    #[serde(default)]
    pub max_parallel: Option<usize>,
    /// How many Runs of each Task are kept on disk.
    #[serde(default)]
    pub max_runs_per_task: Option<usize>,
    /// How much of a Run's output is kept before the log is truncated.
    #[serde(default)]
    pub max_log_bytes: Option<u64>,
    /// The timeout for Tasks that name none.
    #[serde(default)]
    pub default_timeout: Option<String>,
    /// Where the local API listens. Loopback by default; widening it is
    /// possible, discouraged, and never removes the token requirement.
    #[serde(default)]
    pub bind: Option<String>,
    /// The Agent for Tasks that name none. Unset by default, so an omitted
    /// `agent:` is Broken until the user opts in.
    #[serde(default)]
    pub default_agent: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProjectConfig {
    pub path: PathBuf,
    /// Unique short name; defaults to the directory's basename.
    #[serde(default)]
    pub name: Option<String>,
    /// Whether to keep a `CRONTAB.md` at this Project's root. Openroutine
    /// never insists on writing into someone's repository.
    #[serde(default)]
    pub crontab_md: Option<bool>,
    /// Runs kept per Task here, overriding the global setting.
    #[serde(default)]
    pub max_runs_per_task: Option<usize>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentConfig {
    pub cmd: String,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("reading config at {}", path.display()))?;
        let config: Config = toml::from_str(&raw)
            .with_context(|| format!("parsing config at {}", path.display()))?;

        // Resolved eagerly so a bad value fails here, in front of whoever
        // just edited the file, rather than at 2am inside a Run.
        config
            .default_timeout()
            .with_context(|| format!("in config at {}", path.display()))?;

        let mut seen: BTreeMap<String, &Path> = BTreeMap::new();
        for project in &config.projects {
            let name = project.resolved_name();
            // A `/` would make the Task id ambiguous, since ids are
            // `<project>/<task>`.
            if name.is_empty() || name.contains('/') {
                anyhow::bail!(
                    "project name {name:?} ({}) may not be empty or contain `/`",
                    project.path.display()
                );
            }
            if let Some(other) = seen.insert(name.clone(), &project.path) {
                anyhow::bail!(
                    "two projects are both called {name:?} ({} and {}); \
                     give one of them a `name` of its own",
                    other.display(),
                    project.path.display()
                );
            }
        }

        for keep in std::iter::once(config.max_runs_per_task)
            .chain(
                config
                    .projects
                    .iter()
                    .map(|project| project.max_runs_per_task),
            )
            .flatten()
        {
            if keep == 0 {
                anyhow::bail!(
                    "`max_runs_per_task` must be at least 1; a Task keeps its current Run"
                );
            }
        }

        for (name, agent) in &config.agents {
            crate::runner::check_placeholders(&agent.cmd)
                .map_err(|reason| anyhow::anyhow!("agent {name:?}: {reason}"))
                .with_context(|| format!("in config at {}", path.display()))?;
        }

        Ok(config)
    }

    pub fn state_dir(&self) -> Result<PathBuf> {
        match &self.state_dir {
            Some(dir) => Ok(dir.clone()),
            None => default_state_dir(),
        }
    }

    /// The ceiling on concurrent Runs, if there is one.
    pub fn max_parallel(&self) -> Option<usize> {
        self.max_parallel
    }

    /// Runs kept per Task before the oldest are pruned.
    pub fn max_runs_per_task(&self) -> usize {
        self.max_runs_per_task.unwrap_or(DEFAULT_MAX_RUNS_PER_TASK)
    }

    /// Output kept per Run before truncation.
    pub fn max_log_bytes(&self) -> u64 {
        self.max_log_bytes.unwrap_or(DEFAULT_MAX_LOG_BYTES)
    }

    /// The timeout a Task inherits when it names none.
    pub fn default_timeout(&self) -> Result<crate::task::Timeout> {
        match &self.default_timeout {
            Some(text) if text.trim() == "none" => Ok(crate::task::Timeout::Never),
            Some(text) => crate::task::parse_duration(text)
                .map(crate::task::Timeout::After)
                .map_err(|reason| anyhow::anyhow!("`default_timeout`: {reason}")),
            None => Ok(crate::task::Timeout::After(DEFAULT_TIMEOUT)),
        }
    }

    /// The address the API listens on.
    pub fn bind(&self) -> String {
        self.bind
            .clone()
            .unwrap_or_else(|| format!("127.0.0.1:{}", crate::api::DEFAULT_PORT))
    }

    pub fn agent(&self, name: &str) -> Option<&AgentConfig> {
        self.agents.get(name)
    }

    /// Resolves the Agent a Task will run under: the one it names, else the
    /// configured default. The error is what makes the Task Broken.
    pub fn resolve_agent(&self, requested: Option<&str>) -> Result<String, String> {
        let name = requested
            .or(self.default_agent.as_deref())
            .ok_or("no agent: the task names none and the config sets no `default_agent`")?;

        if self.agents.contains_key(name) {
            Ok(name.to_string())
        } else {
            Err(format!(
                "unknown agent {name:?}: the config defines no agent by that name"
            ))
        }
    }
}

impl ProjectConfig {
    /// The Project's name: explicit, else the directory's basename.
    pub fn resolved_name(&self) -> String {
        self.name.clone().unwrap_or_else(|| {
            self.path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_else(|| "project".to_string())
        })
    }
}

/// An XDG base directory, if the environment sets a usable one. The spec says
/// a relative value is invalid and must be treated as unset, so a stray
/// `XDG_STATE_HOME=state` resolves under `$HOME` rather than scattering state
/// relative to whatever directory the daemon happened to start in.
fn xdg_base_dir(variable: &str) -> Option<PathBuf> {
    let value = PathBuf::from(std::env::var_os(variable)?);
    value.is_absolute().then_some(value)
}

/// `$XDG_STATE_HOME/openroutine`, else `~/.local/state/openroutine` — the same
/// path on macOS and Linux, which is the point of owning the scheduler.
pub fn default_state_dir() -> Result<PathBuf> {
    if let Some(xdg) = xdg_base_dir("XDG_STATE_HOME") {
        return Ok(xdg.join("openroutine"));
    }
    let home = std::env::var_os("HOME").context("neither XDG_STATE_HOME nor HOME is set")?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("state")
        .join("openroutine"))
}

/// `$XDG_CONFIG_HOME/openroutine/config.toml`, else `~/.config/...`.
pub fn default_config_path() -> Result<PathBuf> {
    if let Some(xdg) = xdg_base_dir("XDG_CONFIG_HOME") {
        return Ok(xdg.join("openroutine").join("config.toml"));
    }
    let home = std::env::var_os("HOME").context("neither XDG_CONFIG_HOME nor HOME is set")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("openroutine")
        .join("config.toml"))
}
