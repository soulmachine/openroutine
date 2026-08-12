//! The local API: what it serves, and what it refuses.

mod support;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use support::TestEnv;

const BIN: &str = env!("CARGO_BIN_EXE_openroutine");

const MANUAL: &str = "---\ndescription: On demand\nagent: stub\n---\n\nping\n";

struct Served {
    child: std::process::Child,
    base: String,
    token: String,
}

impl Drop for Served {
    fn drop(&mut self) {
        unsafe {
            libc::kill(self.child.id() as libc::pid_t, libc::SIGTERM);
        }
        let _ = self.child.wait();
    }
}

// The child is reaped by `Served`'s Drop; clippy cannot see across it.
#[allow(clippy::zombie_processes)]
fn serve(env: &TestEnv) -> Served {
    let port = env.write_config_with_api();
    let token = env.api_token();
    let child = Command::new(BIN)
        .args(["serve", "--config"])
        .arg(env.config_path())
        .env("XDG_STATE_HOME", env.xdg_state_home())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();

    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if curl(&["-s", "-o", "/dev/null", &format!("{base}/v1/tasks")]).is_some() {
            return Served { child, base, token };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the api never came up");
}

/// Runs curl and returns its stdout, or None if it could not connect.
fn curl(args: &[&str]) -> Option<String> {
    let output = Command::new("curl").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

impl Served {
    fn get(&self, path: &str) -> serde_json::Value {
        let body = curl(&[
            "-s",
            "-H",
            &format!("Authorization: Bearer {}", self.token),
            &format!("{}{path}", self.base),
        ])
        .expect("curl should reach the api");
        serde_json::from_str(&body).unwrap_or(serde_json::Value::String(body))
    }

    /// Returns the status code and parsed body.
    fn send(
        &self,
        method: &str,
        path: &str,
        token: Option<&str>,
        body: Option<&str>,
    ) -> (u16, serde_json::Value) {
        let url = format!("{}{path}", self.base);
        let auth = format!("Authorization: Bearer {}", token.unwrap_or(&self.token));
        let mut args: Vec<String> = vec![
            "-s".into(),
            "-X".into(),
            method.into(),
            "-H".into(),
            auth,
            "-w".into(),
            "\n%{http_code}".into(),
        ];
        if let Some(body) = body {
            args.push("-H".into());
            args.push("Content-Type: application/json".into());
            args.push("-d".into());
            args.push(body.into());
        }
        args.push(url);

        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let raw = curl(&borrowed).expect("curl should reach the api");
        let (body, code) = raw.rsplit_once('\n').unwrap_or(("", "0"));
        (
            code.trim().parse().unwrap_or(0),
            serde_json::from_str(body).unwrap_or(serde_json::Value::String(body.to_string())),
        )
    }
}

#[test]
fn every_endpoint_needs_the_token_including_reads() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    let api = serve(&env);

    for (method, path) in [
        ("GET", "/v1/tasks"),
        ("GET", "/v1/tasks/proj/ondemand"),
        ("GET", "/v1/tasks/proj/ondemand/runs"),
        ("POST", "/v1/tasks/proj/ondemand/fire"),
    ] {
        let (code, body) = api.send(method, path, Some("not-the-token"), None);
        assert_eq!(code, 401, "{method} {path} should refuse: {body}");
        assert_eq!(body["error"]["code"], "unauthorized", "{body}");
    }
}

#[test]
fn the_task_list_says_what_is_scheduled() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    env.write_task(
        "nightly",
        "---\ndescription: Nightly\ncron: \"0 2 * * *\"\nagent: stub\n---\n\nping\n",
    );
    let api = serve(&env);

    let body = api.get("/v1/tasks");
    let tasks = body["tasks"].as_array().unwrap();

    assert_eq!(tasks.len(), 2, "{body}");
    let manual = tasks
        .iter()
        .find(|task| task["id"] == "proj/ondemand")
        .unwrap();
    assert!(
        manual["nextFireAt"].is_null(),
        "a Manual task has no next fire — null, never a placeholder: {manual}"
    );
    let nightly = tasks
        .iter()
        .find(|task| task["id"] == "proj/nightly")
        .unwrap();
    assert!(nightly["nextFireAt"].is_string(), "{nightly}");
}

#[test]
fn firing_a_task_starts_a_run_and_says_where_to_look() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    let api = serve(&env);

    let (code, body) = api.send("POST", "/v1/tasks/proj/ondemand/fire", None, None);

    assert_eq!(code, 202, "{body}");
    assert!(body["run_id"].is_string(), "{body}");
    assert!(
        body["log"].as_str().unwrap().contains("output.log"),
        "{body}"
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && env.calls().is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(env.calls().len(), 1, "the agent actually ran");
}

#[test]
fn fired_context_reaches_the_agent_wrapped_and_labelled() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    let api = serve(&env);

    api.send(
        "POST",
        "/v1/tasks/proj/ondemand/fire",
        None,
        Some(r#"{"text":"Sentry alert SEN-4521 fired in prod."}"#),
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && env.calls().is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }

    let prompt = env.calls()[0].args.last().unwrap().clone();
    assert!(
        prompt.starts_with("ping"),
        "the task's own prompt leads: {prompt}"
    );
    assert!(prompt.contains("<run-context>"), "{prompt}");
    assert!(prompt.contains("SEN-4521"), "{prompt}");
    assert!(
        prompt.contains("not instructions"),
        "the wrapper must say what the text is and is not: {prompt}"
    );
}

#[test]
fn an_oversized_payload_is_refused_rather_than_truncated() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    let api = serve(&env);

    let huge = "x".repeat(70_000);
    let (code, body) = api.send(
        "POST",
        "/v1/tasks/proj/ondemand/fire",
        None,
        Some(&format!(r#"{{"text":"{huge}"}}"#)),
    );

    assert_eq!(code, 413, "{body}");
    assert_eq!(body["error"]["code"], "context_too_large", "{body}");
}

#[test]
fn firing_a_task_that_is_already_running_is_refused() {
    let env = TestEnv::new();
    env.write_gated_stub_agent();
    env.write_task("ondemand", MANUAL);
    let api = serve(&env);

    let (first, _) = api.send("POST", "/v1/tasks/proj/ondemand/fire", None, None);
    assert_eq!(first, 202);

    let (second, body) = api.send("POST", "/v1/tasks/proj/ondemand/fire", None, None);
    assert_eq!(second, 409, "{body}");
    assert_eq!(body["error"]["code"], "already_running", "{body}");

    env.open_gate();
}

#[test]
fn an_unknown_task_is_a_clean_not_found() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    let api = serve(&env);

    let (code, body) = api.send("GET", "/v1/tasks/proj/ghost", None, None);

    assert_eq!(code, 404, "{body}");
    assert_eq!(body["error"]["code"], "no_such_task", "{body}");
}

#[test]
fn run_history_and_logs_are_readable() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    let api = serve(&env);

    api.send("POST", "/v1/tasks/proj/ondemand/fire", None, None);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && env.calls().is_empty() {
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(300));

    let runs = api.get("/v1/tasks/proj/ondemand/runs");
    let listed = runs["runs"].as_array().unwrap();
    assert_eq!(listed.len(), 1, "{runs}");
    assert_eq!(listed[0]["trigger"], "fire", "{runs}");

    let run_id = listed[0]["runId"].as_str().unwrap();
    let log = api.get(&format!("/v1/runs/proj/ondemand/{run_id}/log"));
    assert!(
        log.as_str().unwrap().contains("stub agent ran"),
        "the log comes back as text: {log}"
    );
}

#[test]
fn an_empty_history_is_an_empty_list_not_a_null() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    let api = serve(&env);

    let runs = api.get("/v1/tasks/proj/ondemand/runs");

    assert_eq!(
        runs["runs"].as_array().map(Vec::len),
        Some(0),
        "nothing found is not the same as nothing known: {runs}"
    );
}
