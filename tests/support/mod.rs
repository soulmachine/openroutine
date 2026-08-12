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
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};
use tempfile::TempDir;

pub fn at(iso: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(iso)
        .unwrap_or_else(|e| panic!("bad timestamp {iso:?}: {e}"))
        .with_timezone(&Utc)
}

/// One call of the stub Agent, as the stub itself recorded it.
#[derive(Debug, Clone)]
pub struct AgentCall {
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub stdin: String,
    pub env: std::collections::BTreeMap<String, String>,
}

impl AgentCall {
    pub fn var(&self, name: &str) -> Option<&str> {
        self.env.get(name).map(String::as_str)
    }
}

pub struct TestEnv {
    root: TempDir,
    /// Every sandbox gets its own API port, so daemons in parallel tests
    /// never collide over the default one.
    port: u16,
}

impl TestEnv {
    pub fn new() -> Self {
        let root = TempDir::new().expect("temp root");
        let env = Self {
            root,
            port: free_port(),
        };
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
seq=$(ls {record_dir}/*.args 2>/dev/null | wc -l | tr -d ' ')
record="$(mktemp {record_dir}/inv-$(printf '%04d' "$seq")-XXXXXX)"
for arg in "$@"; do printf '%s\0' "$arg"; done > "$record.args"
pwd > "$record.cwd"
env > "$record.env"
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
seq=$(ls {record_dir}/*.args 2>/dev/null | wc -l | tr -d ' ')
record="$(mktemp {record_dir}/inv-$(printf '%04d' "$seq")-XXXXXX)"
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

    /// A stub Agent that forks a child which outlives it, then hangs. Used to
    /// prove a timeout kills the whole process group, not just the Agent.
    pub fn write_forking_stub_agent(&self, marker: &Path) {
        let script = format!(
            r#"#!/bin/sh
seq=$(ls {record_dir}/*.args 2>/dev/null | wc -l | tr -d ' ')
record="$(mktemp {record_dir}/inv-$(printf '%04d' "$seq")-XXXXXX)"
for arg in "$@"; do printf '%s\0' "$arg"; done > "$record.args"
pwd > "$record.cwd"
env > "$record.env"
: > "$record.stdin"
( sleep 2; : > "{marker}" ) &
while true; do sleep 0.2; done
"#,
            record_dir = self.record_dir().display(),
            marker = marker.display(),
        );
        let path = self.stub_path();
        fs::write(&path, script).unwrap();
        make_executable(&path);
    }

    /// A stub Agent that floods its output.
    pub fn write_noisy_stub_agent(&self, lines: usize) {
        let script = format!(
            r#"#!/bin/sh
seq=$(ls {record_dir}/*.args 2>/dev/null | wc -l | tr -d ' ')
record="$(mktemp {record_dir}/inv-$(printf '%04d' "$seq")-XXXXXX)"
for arg in "$@"; do printf '%s\0' "$arg"; done > "$record.args"
pwd > "$record.cwd"
env > "$record.env"
: > "$record.stdin"
i=0
while [ $i -lt {lines} ]; do
  printf 'noisy line %s ........................................\n' "$i"
  i=$((i+1))
done
"#,
            record_dir = self.record_dir().display(),
            lines = lines,
        );
        let path = self.stub_path();
        fs::write(&path, script).unwrap();
        make_executable(&path);
    }

    /// The standard config plus extra top-level TOML prepended.
    pub fn write_config_with_extra(&self, extra: &str) -> PathBuf {
        self.write_config_toml(
            &format!("{} --run {{prompt}}", self.stub_path().display()),
            extra,
        )
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
            "bind = \"127.0.0.1:{port}\"\n\
             {preamble}\
             [[projects]]\n\
             path = {project:?}\n\
             name = \"proj\"\n\
             \n\
             [agents.stub]\n\
             cmd = {cmd:?}\n",
            port = self.port,
            project = self.project_dir().display().to_string(),
            cmd = cmd,
        );
        let path = self.config_path();
        fs::write(&path, config).unwrap();
        path
    }

    /// Creates another directory under the sandbox, ready to register.
    pub fn add_project(&self, relative: &str) -> PathBuf {
        let path = self.root.path().join(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }

    /// A config registering both the default Project and `other`.
    pub fn write_config_with_projects(&self) -> PathBuf {
        let other = self.add_project("other");
        let config = format!(
            "bind = \"127.0.0.1:{port}\"\n\
             [[projects]]\npath = {proj:?}\nname = \"proj\"\n\n\
             [[projects]]\npath = {other:?}\nname = \"other\"\n\n\
             [agents.stub]\ncmd = {cmd:?}\n",
            port = self.port,
            proj = self.project_dir().display().to_string(),
            other = other.display().to_string(),
            cmd = format!("{} --run {{prompt}}", self.stub_path().display()),
        );
        fs::write(self.config_path(), config).unwrap();
        self.config_path()
    }

    /// The port this sandbox's API listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// A config whose API listens on this sandbox's own port.
    pub fn write_config_with_api(&self) -> u16 {
        self.write_config();
        self.port
    }

    /// The API token the daemon will use, created up front so a test can
    /// authenticate without racing the daemon's own first write.
    pub fn api_token(&self) -> String {
        openroutine::token::load_or_create(&self.state_dir()).unwrap()
    }

    /// A config with agents but no Projects registered yet.
    pub fn write_config_with_no_projects(&self) -> PathBuf {
        let config = format!(
            "bind = \"127.0.0.1:{port}\"\n[agents.stub]\ncmd = {cmd:?}\n",
            port = self.port,
            cmd = format!("{} --run {{prompt}}", self.stub_path().display()),
        );
        fs::write(self.config_path(), config).unwrap();
        self.config_path()
    }

    /// The standard config with an extra key inside the Project entry.
    pub fn write_config_with_extra_project_key(&self, extra: &str) -> PathBuf {
        let config = format!(
            "bind = \"127.0.0.1:{port}\"\n\
             [[projects]]\npath = {proj:?}\nname = \"proj\"\n{extra}\n\
             [agents.stub]\ncmd = {cmd:?}\n",
            port = self.port,
            proj = self.project_dir().display().to_string(),
            extra = extra,
            cmd = format!("{} --run {{prompt}}", self.stub_path().display()),
        );
        fs::write(self.config_path(), config).unwrap();
        self.config_path()
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
    pub fn calls(&self) -> Vec<AgentCall> {
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
            // A record being written right now is not yet a call: the stub
            // writes its files one at a time, and a poll can land between.
            .filter(|args_path| args_path.with_extension("cwd").exists())
            .map(|args_path| {
                let stem = args_path.with_extension("");
                let raw = fs::read(&args_path).unwrap();
                let args = String::from_utf8(raw)
                    .unwrap()
                    .split('\0')
                    .filter(|piece| !piece.is_empty())
                    .map(str::to_string)
                    .collect();
                AgentCall {
                    args,
                    cwd: PathBuf::from(
                        fs::read_to_string(stem.with_extension("cwd"))
                            .unwrap_or_default()
                            .trim()
                            .to_string(),
                    ),
                    stdin: fs::read_to_string(stem.with_extension("stdin")).unwrap_or_default(),
                    env: fs::read_to_string(stem.with_extension("env"))
                        .unwrap_or_default()
                        .lines()
                        .filter_map(|line| line.split_once('='))
                        .map(|(name, value)| (name.to_string(), value.to_string()))
                        .collect(),
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

    /// Runs the real binary with `env` layered on top, for the tests that
    /// need to control what the Daemon's login shell would inherit.
    pub fn run_with_env(&self, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_openroutine"));
        command
            .args(args)
            .arg("--config")
            .arg(self.config_path())
            .env("XDG_STATE_HOME", self.xdg_state_home());
        for (name, value) in env {
            command.env(name, value);
        }
        command.output().expect("binary should run")
    }

    /// Writes an executable that does nothing, and returns the directory
    /// holding it — somewhere to put a program that only some `PATH`s reach.
    pub fn write_program_in(&self, dir: &str, name: &str) -> PathBuf {
        let dir = self.root.path().join(dir);
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
        make_executable(&path);
        dir
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

    /// The state file, or `None` while it is absent or mid-write.
    pub fn try_read_state(&self) -> Option<serde_json::Value> {
        serde_json::from_str(&fs::read_to_string(self.state_file()).ok()?).ok()
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

/// A Daemon running as a real process, reaped when it goes out of scope.
///
/// `std::process::Child` deliberately does not kill on drop, so a test that
/// panicked between spawning a Daemon and stopping it left one running: it
/// outlives the test binary, holds the lock on its state directory, and is
/// invisible until somebody goes looking. A panic between those two points
/// is precisely what a failing assertion does — so the leak appeared only
/// when the suite was red, which is exactly when a stray Daemon writing
/// state is least welcome and least likely to be noticed.
///
/// Reaping is therefore unconditional. An explicit stop still hands back
/// what a test needs to assert on; everything else is caught on the way out.
pub struct DaemonProcess {
    child: Child,
    /// Whether the child has been waited on. Signalling a reaped pid could
    /// reach whatever the operating system has since reused the number for,
    /// so this is tracked rather than risked.
    reaped: bool,
}

impl DaemonProcess {
    /// Spawns `serve` against this sandbox — its config, and its state dir.
    pub fn spawn(env: &TestEnv) -> Self {
        Self::spawn_with(
            Command::new(env!("CARGO_BIN_EXE_openroutine"))
                .args(["serve", "--config"])
                .arg(env.config_path())
                .env("XDG_STATE_HOME", env.xdg_state_home()),
        )
    }

    /// Spawns an already-configured command, for the tests that need an
    /// environment of their own. Output is discarded either way: a daemon
    /// logging into a pipe nobody drains would eventually block on it.
    pub fn spawn_with(command: &mut Command) -> Self {
        let child = command
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("daemon should spawn");
        Self {
            child,
            reaped: false,
        }
    }

    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// Whether it has exited already, without waiting on it.
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        self.child.try_wait().unwrap()
    }

    /// Stops it the way a service manager does, and reaps it. Fails rather
    /// than hanging if the signal is ignored.
    pub fn stop(&mut self) -> ExitStatus {
        self.signal(libc::SIGTERM);
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(status) = self.try_wait() {
                self.reaped = true;
                return status;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.reaped = true;
        panic!("daemon did not exit after SIGTERM");
    }

    /// Kills it outright, as a crash would, and reaps it.
    pub fn kill(&mut self) {
        self.signal(libc::SIGKILL);
        let _ = self.child.wait();
        self.reaped = true;
    }

    fn signal(&self, signal: libc::c_int) {
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, signal);
        }
    }
}

impl Drop for DaemonProcess {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        self.signal(libc::SIGTERM);
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = fs::metadata(path).unwrap().permissions();
    perms.set_mode(0o755);
    fs::set_permissions(path, perms).unwrap();
}

/// A port the operating system says is free right now.
pub fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap().port()
}
