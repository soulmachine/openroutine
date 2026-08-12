//! The local HTTP API.
//!
//! Bound to loopback and guarded by a bearer token on every endpoint, reads
//! included — the task list carries the prompts you run, which is no less
//! sensitive than firing them.

use crate::daemon::{Daemon, FireOutcome};
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::Mutex;

pub const DEFAULT_PORT: u16 = 7373;

#[derive(Clone)]
pub struct Api {
    pub daemon: Arc<Mutex<Daemon>>,
    pub token: String,
}

pub fn router(api: Api) -> Router {
    Router::new()
        .route("/v1/tasks", get(list_tasks))
        .route("/v1/tasks/{project}/{name}", get(task_detail))
        .route("/v1/tasks/{project}/{name}/runs", get(task_runs))
        .route("/v1/tasks/{project}/{name}/fire", post(fire))
        .route("/v1/runs/{project}/{name}/{run}", get(run_detail))
        .route("/v1/runs/{project}/{name}/{run}/log", get(run_log))
        .with_state(api)
}

/// Anything that went wrong, in the one shape every endpoint uses.
struct Failure(StatusCode, &'static str, String);

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        let Failure(status, code, message) = self;
        (
            status,
            Json(json!({ "error": { "code": code, "message": message } })),
        )
            .into_response()
    }
}

fn authorise(api: &Api, headers: &HeaderMap) -> Result<(), Failure> {
    let offered = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .unwrap_or_default()
        .trim();

    if crate::token::matches(&api.token, offered) {
        Ok(())
    } else {
        Err(Failure(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "a valid bearer token is required; see `openroutine token`".to_string(),
        ))
    }
}

async fn list_tasks(State(api): State<Api>, headers: HeaderMap) -> Result<Json<Value>, Failure> {
    authorise(&api, &headers)?;
    let daemon = api.daemon.lock().await;
    let tasks: Vec<Value> = daemon
        .scheduled_tasks()
        .iter()
        .map(|task| summarise(&daemon, task))
        .collect();
    Ok(Json(json!({ "tasks": tasks })))
}

async fn task_detail(
    State(api): State<Api>,
    headers: HeaderMap,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<Value>, Failure> {
    authorise(&api, &headers)?;
    let id = format!("{project}/{name}");
    let daemon = api.daemon.lock().await;

    let summary = daemon
        .scheduled_tasks()
        .into_iter()
        .find(|task| task.id == id)
        .ok_or_else(|| unknown_task(&id))?;

    let mut body = summarise(&daemon, &summary);
    // Embedded rather than behind another endpoint: the reason a Task did
    // not run belongs beside the Task.
    body["skips"] = json!(
        daemon
            .state()
            .recorded_skips
            .get(&id)
            .cloned()
            .unwrap_or_default()
    );
    Ok(Json(body))
}

fn summarise(daemon: &Daemon, task: &crate::daemon::ScheduledSummary) -> Value {
    let state = daemon.state();
    json!({
        "id": task.id,
        // Null, never a placeholder date: a Manual or Completed Task has no
        // next fire, and saying so is not the same as saying "the year 1".
        "nextTick": task.next_tick,
        "nextFireAt": task.next_fire_at,
        "running": daemon.is_running(&task.id),
        "lastRunAt": state.last_run_at(&task.id),
        "lastScheduledFor": state.last_scheduled_for(&task.id),
    })
}

fn unknown_task(id: &str) -> Failure {
    Failure(
        StatusCode::NOT_FOUND,
        "no_such_task",
        format!("no task called {id:?}"),
    )
}

#[derive(Debug, Deserialize)]
struct Paging {
    #[serde(default)]
    limit: Option<usize>,
}

async fn task_runs(
    State(api): State<Api>,
    headers: HeaderMap,
    Path((project, name)): Path<(String, String)>,
    Query(paging): Query<Paging>,
) -> Result<Json<Value>, Failure> {
    authorise(&api, &headers)?;
    let id = format!("{project}/{name}");
    let daemon = api.daemon.lock().await;
    let mut runs = crate::run::read_history(daemon.state_dir(), &id);
    runs.reverse(); // newest first
    runs.truncate(paging.limit.unwrap_or(50));
    Ok(Json(json!({ "runs": runs })))
}

async fn run_detail(
    State(api): State<Api>,
    headers: HeaderMap,
    Path((project, name, run)): Path<(String, String, String)>,
) -> Result<Json<Value>, Failure> {
    authorise(&api, &headers)?;
    let id = format!("{project}/{name}");
    let daemon = api.daemon.lock().await;
    crate::run::read_record(daemon.state_dir(), &id, &run)
        .map(|record| Json(json!(record)))
        .ok_or_else(|| {
            Failure(
                StatusCode::NOT_FOUND,
                "no_such_run",
                format!("no run {run:?} for {id:?}"),
            )
        })
}

async fn run_log(
    State(api): State<Api>,
    headers: HeaderMap,
    Path((project, name, run)): Path<(String, String, String)>,
) -> Result<Response, Failure> {
    authorise(&api, &headers)?;
    let id = format!("{project}/{name}");
    let daemon = api.daemon.lock().await;
    let path = crate::run::task_runs_dir(daemon.state_dir(), &id)
        .join(&run)
        .join(crate::run::RUN_LOG);

    std::fs::read_to_string(&path)
        .map(|body| {
            (
                [(
                    axum::http::header::CONTENT_TYPE,
                    "text/plain; charset=utf-8",
                )],
                body,
            )
                .into_response()
        })
        .map_err(|_| {
            Failure(
                StatusCode::NOT_FOUND,
                "no_such_run",
                format!("no log for run {run:?} of {id:?}"),
            )
        })
}

#[derive(Debug, Default, Deserialize)]
struct FireBody {
    #[serde(default)]
    text: Option<String>,
}

#[derive(Serialize)]
struct Fired {
    run_id: String,
    log: String,
}

async fn fire(
    State(api): State<Api>,
    headers: HeaderMap,
    Path((project, name)): Path<(String, String)>,
    body: Option<Json<FireBody>>,
) -> Result<(StatusCode, Json<Value>), Failure> {
    authorise(&api, &headers)?;
    let id = format!("{project}/{name}");
    let text = body.and_then(|Json(body)| body.text);

    if let Some(text) = &text
        && text.len() > crate::fire::MAX_CONTEXT_BYTES
    {
        return Err(Failure(
            StatusCode::PAYLOAD_TOO_LARGE,
            "context_too_large",
            format!(
                "context is {} bytes; the limit is {}",
                text.len(),
                crate::fire::MAX_CONTEXT_BYTES
            ),
        ));
    }

    let mut daemon = api.daemon.lock().await;
    match daemon.fire(&id, text.as_deref()) {
        FireOutcome::Started(run_id) => {
            let log = crate::run::task_runs_dir(daemon.state_dir(), &id)
                .join(&run_id)
                .join(crate::run::RUN_LOG);
            Ok((
                StatusCode::ACCEPTED,
                Json(json!(Fired {
                    run_id,
                    log: log.display().to_string()
                })),
            ))
        }
        FireOutcome::AlreadyRunning => Err(Failure(
            StatusCode::CONFLICT,
            "already_running",
            format!("{id} is already running; cancel it first if you mean to replace it"),
        )),
        FireOutcome::NoSuchTask => Err(unknown_task(&id)),
        FireOutcome::Failed(reason) => Err(Failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could_not_start",
            reason,
        )),
    }
}
