//! The web UI, from outside: what is served, and what it refuses.
//!
//! Coverage stops at the process boundary by design — the page's own
//! behaviour is checked by hand rather than by a browser harness.

mod support;

use std::process::Command;
use std::time::{Duration, Instant};
use support::{DaemonProcess, TestEnv};

// A schedule far enough out that nothing fires on its own during a
// test: these are about firing on demand, not about the clock.
const MANUAL: &str = "---\ndescription: On demand\ncron: \"0 4 1 1 *\"\nagent: stub\n---\n\nping\n";

struct Served {
    /// Held for its Drop: the daemon is reaped even if a test panics.
    _daemon: DaemonProcess,
    base: String,
    token: String,
}

fn serve(env: &TestEnv) -> Served {
    let port = env.write_config_with_api();
    let token = env.api_token();
    let _daemon = DaemonProcess::spawn(env);

    let base = format!("http://127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if raw(&["-s", "-o", "/dev/null", &format!("{base}/")]).is_some() {
            return Served {
                _daemon,
                base,
                token,
            };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the ui never came up");
}

fn raw(args: &[&str]) -> Option<String> {
    let output = Command::new("curl").args(args).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).to_string())
}

#[test]
fn the_page_is_served_from_the_binary_with_a_strict_policy() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    let ui = serve(&env);

    let headers = raw(&["-s", "-D", "-", "-o", "/dev/null", &format!("{}/", ui.base)]).unwrap();
    let body = raw(&["-s", &format!("{}/", ui.base)]).unwrap();

    assert!(headers.to_lowercase().contains("text/html"), "{headers}");
    assert!(
        headers.to_lowercase().contains("content-security-policy"),
        "the no-CDN rule should be enforced by the browser, not just intended:\n{headers}"
    );
    assert!(
        headers.contains("default-src 'self'"),
        "and it should be same-origin only:\n{headers}"
    );
    assert!(body.contains("OpenRoutine"), "{body}");
}

#[test]
fn nothing_on_the_page_comes_from_anywhere_else() {
    let env = TestEnv::new();
    let ui = serve(&env);

    let page = raw(&["-s", &format!("{}/", ui.base)]).unwrap();
    let script = raw(&["-s", &format!("{}/assets/app.js", ui.base)]).unwrap();
    let styles = raw(&["-s", &format!("{}/assets/styles.css", ui.base)]).unwrap();

    for (what, text) in [("page", &page), ("script", &script), ("styles", &styles)] {
        assert!(
            !text.contains("http://") && !text.contains("https://"),
            "the {what} must work offline, with no external reference"
        );
    }
    assert!(script.contains("/v1/tasks"), "the script talks to our api");
}

#[test]
fn the_assets_are_typed_so_a_browser_will_run_them() {
    let env = TestEnv::new();
    let ui = serve(&env);

    let script = raw(&[
        "-s",
        "-D",
        "-",
        "-o",
        "/dev/null",
        &format!("{}/assets/app.js", ui.base),
    ])
    .unwrap();
    let styles = raw(&[
        "-s",
        "-D",
        "-",
        "-o",
        "/dev/null",
        &format!("{}/assets/styles.css", ui.base),
    ])
    .unwrap();

    assert!(
        script.to_lowercase().contains("text/javascript"),
        "{script}"
    );
    assert!(
        script.to_lowercase().contains("content-security-policy"),
        "{script}"
    );
    assert!(styles.to_lowercase().contains("text/css"), "{styles}");
}

#[test]
fn the_one_time_link_signs_the_page_in() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    let ui = serve(&env);

    let headers = raw(&[
        "-s",
        "-D",
        "-",
        "-o",
        "/dev/null",
        &format!("{}/?token={}", ui.base, ui.token),
    ])
    .unwrap();

    assert!(
        headers.contains("302") || headers.contains("303"),
        "{headers}"
    );
    assert!(headers.to_lowercase().contains("set-cookie"), "{headers}");
    assert!(
        headers.contains("HttpOnly") && headers.contains("SameSite=Strict"),
        "the session cookie should be as narrow as it can be:\n{headers}"
    );
    assert!(
        headers.to_lowercase().contains("location: /"),
        "and the token should not stay in the address bar:\n{headers}"
    );
}

#[test]
fn a_wrong_link_signs_nobody_in() {
    let env = TestEnv::new();
    let ui = serve(&env);

    let headers = raw(&[
        "-s",
        "-D",
        "-",
        "-o",
        "/dev/null",
        &format!("{}/?token=not-the-token", ui.base),
    ])
    .unwrap();

    assert!(headers.contains("401"), "{headers}");
    assert!(!headers.to_lowercase().contains("set-cookie"), "{headers}");
}

#[test]
fn the_session_cookie_is_accepted_by_the_api() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    let ui = serve(&env);

    let body = raw(&[
        "-s",
        "-H",
        &format!("Cookie: openroutine_session={}", ui.token),
        &format!("{}/v1/tasks", ui.base),
    ])
    .unwrap();

    assert!(body.contains("ondemand"), "{body}");
}

#[test]
fn a_page_with_no_session_gets_nothing() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    let ui = serve(&env);

    let body = raw(&["-s", &format!("{}/v1/tasks", ui.base)]).unwrap();

    assert!(body.contains("unauthorized"), "{body}");
    assert!(!body.contains("ondemand"), "{body}");
}

#[test]
fn the_task_list_carries_what_the_page_needs_to_show() {
    let env = TestEnv::new();
    env.write_task("ondemand", MANUAL);
    env.write_task(
        "once",
        "---\ndescription: Once\nat: \"2026-12-25T09:00:00Z\"\nagent: stub\n---\n\nping\n",
    );
    let ui = serve(&env);

    let body: serde_json::Value = serde_json::from_str(
        &raw(&[
            "-s",
            "-H",
            &format!("Cookie: openroutine_session={}", ui.token),
            &format!("{}/v1/tasks", ui.base),
        ])
        .unwrap(),
    )
    .unwrap();

    let once = body["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|task| task["id"] == "once")
        .unwrap();

    assert_eq!(once["oneShot"], true, "so the page can show a countdown");
    assert!(once["description"].is_string(), "{once}");
    assert!(once["schedule"].is_string(), "{once}");
    assert!(
        once["path"].as_str().unwrap().ends_with("once.md"),
        "the page shows the file to edit rather than an editor: {once}"
    );
}
