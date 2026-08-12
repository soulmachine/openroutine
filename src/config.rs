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
    /// How much of a Run's output is kept before the log is truncated.
    #[serde(default)]
    pub max_log_bytes: Option<u64>,
    /// The timeout for Tasks that name none.
    #[serde(default)]
    pub default_timeout: Option<String>,
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

/// `$XDG_STATE_HOME/openroutine`, else `~/.local/state/openroutine` — the same
/// path on macOS and Linux, which is the point of owning the scheduler.
pub fn default_state_dir() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_STATE_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(xdg).join("openroutine"));
    }
    let home = std::env::var_os("HOME").context("neither XDG_STATE_HOME nor HOME is set")?;
    Ok(PathBuf::from(home)
        .join(".local")
        .join("state")
        .join("openroutine"))
}

/// `$XDG_CONFIG_HOME/openroutine/config.toml`, else `~/.config/...`.
pub fn default_config_path() -> Result<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(xdg).join("openroutine").join("config.toml"));
    }
    let home = std::env::var_os("HOME").context("neither XDG_CONFIG_HOME nor HOME is set")?;
    Ok(PathBuf::from(home)
        .join(".config")
        .join("openroutine")
        .join("config.toml"))
}
