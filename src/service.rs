//! Registering the Daemon with the system's service manager.
//!
//! For supervision only: the OS starts one process and restarts it if it
//! dies, and schedules nothing. Per-user and sudo-free on both platforms —
//! a scheduler for your own tasks should not need root to install.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

pub const SERVICE_NAME: &str = "dev.openroutine.daemon";

/// What would be written, and where.
pub struct ServiceDefinition {
    pub path: PathBuf,
    pub contents: String,
    /// What to run afterwards to make the service manager notice.
    pub activate: Vec<Vec<String>>,
    pub deactivate: Vec<Vec<String>>,
}

/// Builds the definition for this platform.
pub fn definition(binary: &Path, config: &Path, home: &Path) -> Result<ServiceDefinition> {
    // The profile the Daemon will hand to Agents comes from this shell, and
    // a service manager starts processes without one — so it is written into
    // the definition rather than left to chance.
    let shell = std::env::var("SHELL")
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/sh".to_string());

    #[cfg(target_os = "macos")]
    {
        Ok(launch_agent(binary, config, home, &shell))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(systemd_unit(binary, config, home, &shell))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (binary, config, home, shell);
        anyhow::bail!("openroutine installs a service on macOS and Linux only")
    }
}

#[cfg(target_os = "macos")]
fn launch_agent(binary: &Path, config: &Path, home: &Path, shell: &str) -> ServiceDefinition {
    let path = home
        .join("Library/LaunchAgents")
        .join(format!("{SERVICE_NAME}.plist"));
    let target = format!("gui/{}", unsafe { libc::getuid() });

    let contents = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{SERVICE_NAME}</string>
  <key>ProgramArguments</key>
  <array>
    <string>{binary}</string>
    <string>serve</string>
    <string>--config</string>
    <string>{config}</string>
  </array>
  <key>EnvironmentVariables</key>
  <dict>
    <key>SHELL</key><string>{shell}</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
"#,
        binary = binary.display(),
        config = config.display(),
    );

    ServiceDefinition {
        activate: vec![
            vec![
                "launchctl".into(),
                "bootstrap".into(),
                target.clone(),
                path.display().to_string(),
            ],
            vec![
                "launchctl".into(),
                "enable".into(),
                format!("{target}/{SERVICE_NAME}"),
            ],
        ],
        deactivate: vec![vec![
            "launchctl".into(),
            "bootout".into(),
            format!("{target}/{SERVICE_NAME}"),
        ]],
        path,
        contents,
    }
}

#[cfg(target_os = "linux")]
fn systemd_unit(binary: &Path, config: &Path, home: &Path, shell: &str) -> ServiceDefinition {
    let path = home
        .join(".config/systemd/user")
        .join(format!("{SERVICE_NAME}.service"));

    let contents = format!(
        "[Unit]\n\
         Description=OpenRoutine — your AI agents' crontab\n\
         After=network.target\n\
         \n\
         [Service]\n\
         ExecStart={binary} serve --config {config}\n\
         Environment=SHELL={shell}\n\
         Restart=always\n\
         RestartSec=5\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        binary = binary.display(),
        config = config.display(),
    );

    ServiceDefinition {
        // Lingering is what makes "starts at boot, no login session" true.
        activate: vec![
            vec!["systemctl".into(), "--user".into(), "daemon-reload".into()],
            vec![
                "systemctl".into(),
                "--user".into(),
                "enable".into(),
                "--now".into(),
                format!("{SERVICE_NAME}.service"),
            ],
            vec!["loginctl".into(), "enable-linger".into()],
        ],
        deactivate: vec![vec![
            "systemctl".into(),
            "--user".into(),
            "disable".into(),
            "--now".into(),
            format!("{SERVICE_NAME}.service"),
        ]],
        path,
        contents,
    }
}

/// Writes the definition and asks the service manager to pick it up.
pub fn install(definition: &ServiceDefinition) -> Result<()> {
    if let Some(parent) = definition.path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&definition.path, &definition.contents)
        .with_context(|| format!("writing {}", definition.path.display()))?;

    // Reinstalling is ordinary — take the old registration out first so the
    // manager picks up the new definition rather than keeping the old one.
    for command in &definition.deactivate {
        let _ = run_quietly(command);
    }
    for command in &definition.activate {
        run_quietly(command).with_context(|| format!("running `{}`", command.join(" ")))?;
    }
    Ok(())
}

/// Takes the registration out and removes the file, leaving config, state,
/// and Tasks untouched.
pub fn uninstall(definition: &ServiceDefinition) -> Result<bool> {
    for command in &definition.deactivate {
        let _ = run_quietly(command);
    }
    match std::fs::remove_file(&definition.path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("removing {}", definition.path.display())),
    }
}

fn run_quietly(command: &[String]) -> Result<()> {
    let (program, arguments) = command.split_first().context("empty command")?;
    let output = std::process::Command::new(program)
        .args(arguments)
        .output()
        .with_context(|| format!("running {program}"))?;

    if output.status.success() {
        Ok(())
    } else {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim())
    }
}
