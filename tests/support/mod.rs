//! Shared harness for the two agreed seams.
//!
//! Everything lives under one temp root — config, state, and projects — so
//! tests never touch the developer's real XDG directories, and a stub Agent
//! command template stands in for a real coding agent. The stub is not a mock
//! of an internal collaborator: an Agent *is* a command template, so this is
//! the product's own public contract being exercised.

#![allow(dead_code)]

use chrono::{DateTime, Utc};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

pub fn at(iso: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(iso)
        .unwrap_or_else(|e| panic!("bad timestamp {iso:?}: {e}"))
        .with_timezone(&Utc)
}

/// One invocation of the stub Agent, as the stub itself recorded it.
#[derive(Debug, Clone)]
pub struct Invocation {
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub stdin: String,
}

pub struct TestEnv {
    root: TempDir,
}

impl TestEnv {
    pub fn new() -> Self {
        let root = TempDir::new().expect("temp root");
        let env = Self { root };
        fs::create_dir_all(env.project_dir()).unwrap();
        fs::create_dir_all(env.state_dir()).unwrap();
        fs::create_dir_all(env.record_dir()).unwrap();
        env.write_stub_agent(0);
        env
    }

    pub fn path(&self) -> &Path {
        self.root.path()
    }

    pub fn project_dir(&self) -> PathBuf {
        self.root.path().join("proj")
    }

    /// The sandbox's `XDG_STATE_HOME`. Spawned daemons get this in their
    /// environment so they resolve state exactly as they would in production,
    /// while staying inside the temp root.
    pub fn xdg_state_home(&self) -> PathBuf {
        self.root.path().join("xdg-state")
    }

    /// Where the daemon keeps machine-owned state, under the sandbox's XDG
    /// state home — the same path the binary resolves for itself.
    pub fn state_dir(&self) -> PathBuf {
        self.xdg_state_home().join("openroutine")
    }

    pub fn record_dir(&self) -> PathBuf {
        self.root.path().join("records")
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.path().join("config.toml")
    }

    pub fn stub_path(&self) -> PathBuf {
        self.root.path().join("stub-agent.sh")
    }

    /// Writes the stub Agent. It records argv NUL-separated (so prompts
    /// containing newlines survive intact), plus cwd and stdin, then exits
    /// with `exit_code`.
    pub fn write_stub_agent(&self, exit_code: i32) {
        let script = format!(
            r#"#!/bin/sh
record="$(mktemp {record_dir}/inv-XXXXXXXX)"
for arg in "$@"; do printf '%s\0' "$arg"; done > "$record.args"
pwd > "$record.cwd"
cat > "$record.stdin"
printf 'stub agent ran\n'
printf 'a line on stderr\n' >&2
exit {exit_code}
"#,
            record_dir = self.record_dir().display(),
            exit_code = exit_code,
        );
        let path = self.stub_path();
        fs::write(&path, script).unwrap();
        make_executable(&path);
    }

    /// A stub Agent that blocks until [`TestEnv::open_gate`] is called, so a
    /// Run can be observed mid-flight without racing a sleep.
    pub fn write_gated_stub_agent(&self) {
        let script = format!(
            r#"#!/bin/sh
record="$(mktemp {record_dir}/inv-XXXXXXXX)"
for arg in "$@"; do printf '%s\0' "$arg"; done > "$record.args"
pwd > "$record.cwd"
: > "$record.stdin"
while [ ! -f "{gate}" ]; do sleep 0.02; done
printf 'stub agent ran\n'
"#,
            record_dir = self.record_dir().display(),
            gate = self.gate_path().display(),
        );
        let path = self.stub_path();
        fs::write(&path, script).unwrap();
        make_executable(&path);
    }

    pub fn gate_path(&self) -> PathBuf {
        self.root.path().join("gate")
    }

    /// Releases a gated stub Agent.
    pub fn open_gate(&self) {
        fs::write(self.gate_path(), "go").unwrap();
    }

    pub fn write_task(&self, name: &str, contents: &str) -> PathBuf {
        let path = self.project_dir().join(format!("{name}.cron.md"));
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, contents).unwrap();
        path
    }

    /// A config whose only agent is the stub, invoked with the prompt as argv.
    pub fn write_config(&self) -> PathBuf {
        self.write_config_with_agent(&format!("{} --run {{prompt}}", self.stub_path().display()))
    }

    /// A config that names the stub as the Agent to use when a Task omits one.
    pub fn write_config_with_default_agent(&self) -> PathBuf {
        self.write_config_toml(
            &format!("{} --run {{prompt}}", self.stub_path().display()),
            "default_agent = \"stub\"\n",
        )
    }

    pub fn write_config_with_agent(&self, cmd: &str) -> PathBuf {
        self.write_config_toml(cmd, "")
    }

    fn write_config_toml(&self, cmd: &str, preamble: &str) -> PathBuf {
        let config = format!(
            "{preamble}\
             [[projects]]\n\
             path = {project:?}\n\
             name = \"proj\"\n\
             \n\
             [agents.stub]\n\
             cmd = {cmd:?}\n",
            project = self.project_dir().display().to_string(),
            cmd = cmd,
        );
        let path = self.config_path();
        fs::write(&path, config).unwrap();
        path
    }

    /// Writes a second copy of the stub Agent at `name`, for cases that need
    /// an executable path the template must quote.
    pub fn write_stub_agent_named(&self, name: &str) -> PathBuf {
        let source = fs::read_to_string(self.stub_path()).unwrap();
        let path = self.root.path().join(name);
        fs::write(&path, source).unwrap();
        make_executable(&path);
        path
    }

    /// The config as the daemon would load it, pointed at this sandbox's
    /// state directory. Storage locations aren't config-file keys, so
    /// in-process callers set the resolved path directly.
    pub fn load_config(&self) -> openroutine::config::Config {
        let mut config =
            openroutine::config::Config::load(&self.config_path()).expect("config should load");
        config.state_dir = Some(self.state_dir());
        config
    }

    /// Every stub invocation, oldest first.
    pub fn invocations(&self) -> Vec<Invocation> {
        let mut records: Vec<PathBuf> = fs::read_dir(self.record_dir())
            .unwrap()
            .filter_map(|entry| {
                let path = entry.unwrap().path();
                (path.extension()? == "args").then_some(path)
            })
            .collect();
        records.sort();

        records
            .into_iter()
            .map(|args_path| {
                let stem = args_path.with_extension("");
                let raw = fs::read(&args_path).unwrap();
                let args = String::from_utf8(raw)
                    .unwrap()
                    .split('\0')
                    .filter(|piece| !piece.is_empty())
                    .map(str::to_string)
                    .collect();
                Invocation {
                    args,
                    cwd: PathBuf::from(
                        fs::read_to_string(stem.with_extension("cwd"))
                            .unwrap()
                            .trim()
                            .to_string(),
                    ),
                    stdin: fs::read_to_string(stem.with_extension("stdin")).unwrap(),
                }
            })
            .collect()
    }

    /// Runs the real binary against this sandbox and captures its output.
    pub fn run(&self, args: &[&str]) -> std::process::Output {
        std::process::Command::new(env!("CARGO_BIN_EXE_openroutine"))
            .args(args)
            .arg("--config")
            .arg(self.config_path())
            .env("XDG_STATE_HOME", self.xdg_state_home())
            .output()
            .expect("binary should run")
    }

    /// Runs the binary and returns its stdout, requiring success.
    pub fn run_ok(&self, args: &[&str]) -> String {
        let output = self.run(args);
        assert!(
            output.status.success(),
            "`openroutine {}` failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("stdout should be utf-8")
    }

    pub fn state_file(&self) -> PathBuf {
        self.state_dir().join("scheduled-tasks.json")
    }

    pub fn read_state(&self) -> serde_json::Value {
        let raw = fs::read_to_string(self.state_file()).expect("state file should exist");
        serde_json::from_str(&raw).expect("state file should be valid json")
    }

    /// Run directories for a task, oldest first.
    pub fn run_dirs(&self, task_id: &str) -> Vec<PathBuf> {
        let (project, name) = task_id
            .split_once('/')
            .expect("task id is <project>/<name>");
        let dir = self.state_dir().join("runs").join(project).join(name);
        let Ok(entries) = fs::read_dir(&dir) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = entries.map(|e| e.unwrap().path()).collect();
        dirs.sort();
        dirs
    }

    pub fn read_run(&self, task_id: &str, index: usize) -> serde_json::Value {
        let dir = &self.run_dirs(task_id)[index];
        let raw = fs::read_to_string(dir.join("run.json")).expect("run.json should exist");
        serde_json::from_str(&raw).expect("run.json should be valid json")
    }

    pub fn read_run_log(&self, task_id: &str, index: usize) -> String {
        let dir = &self.run_dirs(task_id)[index];
        fs::read_to_string(dir.join("output.log")).expect("output.log should exist")
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}
