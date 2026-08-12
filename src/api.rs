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
        .route(
            "/v1/runs/{project}/{name}/{run}/log/stream",
            get(run_log_stream),
        )
        .route("/v1/runs/{project}/{name}/{run}/cancel", post(cancel))
        .route("/v1/tasks/{project}/{name}/pause", post(pause_task))
        .route("/v1/tasks/{project}/{name}/resume", post(resume_task))
        .route("/v1/pause", post(pause_all))
        .route("/v1/resume", post(resume_all))
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
    let bearer = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);

    // The UI holds the same token in a session cookie: same secret, same
    // origin, just the shape a browser can send.
    let cookie = headers
        .get(axum::http::header::COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|jar| {
            jar.split(';').find_map(|pair| {
                pair.trim()
                    .strip_prefix(&format!("{}=", crate::ui::SESSION_COOKIE))
            })
        });

    let offered = bearer.or(cookie).unwrap_or_default();

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
    let mut tasks: Vec<Value> = daemon
        .scheduled_tasks()
        .iter()
        .map(|task| summarise(&daemon, task))
        .collect();

    // A Task that cannot run belongs in the list more than most.
    tasks.extend(
        daemon
            .broken_tasks()
            .into_iter()
            .map(|(id, error, description)| {
                json!({
                    "id": id,
                    "description": description,
                    "error": error,
                    "broken": true,
                    "schedule": null,
                    "nextTick": null,
                    "nextFireAt": null,
                    "running": false,
                    "paused": false,
                    "completed": false,
                })
            }),
    );
    tasks.extend(
        daemon
            .disabled_tasks()
            .into_iter()
            .map(|(id, description)| {
                json!({
                    "id": id,
                    "description": description,
                    "disabled": true,
                    "schedule": null,
                    "nextTick": null,
                    "nextFireAt": null,
                    "running": false,
                    "paused": false,
                    "completed": false,
                })
            }),
    );
    tasks.sort_by(|a, b| a["id"].as_str().cmp(&b["id"].as_str()));
    Ok(Json(
        json!({ "tasks": tasks, "paused": daemon.is_paused(None) }),
    ))
}

async fn task_detail(
    State(api): State<Api>,
    headers: HeaderMap,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<Value>, Failure> {
    authorise(&api, &headers)?;
    let id = format!("{project}/{name}");
    let daemon = api.daemon.lock().await;

    // A Task that cannot run is addressable too — 404-ing the one you most
    // need to look at would be perverse.
    let mut body = match daemon
        .scheduled_tasks()
        .into_iter()
        .find(|task| task.id == id)
    {
        Some(summary) => summarise(&daemon, &summary),
        None => match daemon
            .broken_tasks()
            .into_iter()
            .find(|(other, _, _)| other == &id)
        {
            Some((_, error, description)) => {
                json!({ "id": id, "description": description, "error": error, "broken": true })
            }
            None => match daemon
                .disabled_tasks()
                .into_iter()
                .find(|(other, _)| other == &id)
            {
                Some((_, description)) => {
                    json!({ "id": id, "description": description, "disabled": true })
                }
                None => return Err(unknown_task(&id)),
            },
        },
    };
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
        "description": task.description,
        "novelty": task.novelty,
        "schedule": task.schedule,
        "path": task.path,
        "oneShot": task.one_shot,
        "paused": state.is_paused(&task.id),
        "completed": task.one_shot && task.next_fire_at.is_none(),
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
    // Bounded whatever is asked for: a caller should not be able to make the
    // daemon read ten thousand records into memory.
    runs.truncate(paging.limit.unwrap_or(50).min(500));
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
        FireOutcome::Paused => Err(Failure(
            StatusCode::CONFLICT,
            "paused",
            format!("{id} is paused; resume it before firing"),
        )),
        FireOutcome::Disabled => Err(Failure(
            StatusCode::CONFLICT,
            "disabled",
            format!("{id} is disabled in its own file; remove `disabled: true` to run it"),
        )),
        FireOutcome::NoSuchTask => Err(unknown_task(&id)),
        FireOutcome::Failed(reason) => Err(Failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could_not_start",
            reason,
        )),
    }
}

async fn pause_task(
    State(api): State<Api>,
    headers: HeaderMap,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<Value>, Failure> {
    hold(api, headers, Some(format!("{project}/{name}")), true).await
}

async fn resume_task(
    State(api): State<Api>,
    headers: HeaderMap,
    Path((project, name)): Path<(String, String)>,
) -> Result<Json<Value>, Failure> {
    hold(api, headers, Some(format!("{project}/{name}")), false).await
}

async fn pause_all(State(api): State<Api>, headers: HeaderMap) -> Result<Json<Value>, Failure> {
    hold(api, headers, None, true).await
}

async fn resume_all(State(api): State<Api>, headers: HeaderMap) -> Result<Json<Value>, Failure> {
    hold(api, headers, None, false).await
}

async fn hold(
    api: Api,
    headers: HeaderMap,
    task: Option<String>,
    paused: bool,
) -> Result<Json<Value>, Failure> {
    authorise(&api, &headers)?;
    let mut daemon = api.daemon.lock().await;

    if let Some(id) = &task
        && !daemon.scheduled_tasks().iter().any(|entry| &entry.id == id)
    {
        return Err(unknown_task(id));
    }

    daemon
        .set_paused(task.as_deref(), paused)
        .map_err(|error| {
            Failure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "could_not_save",
                format!("{error:#}"),
            )
        })?;

    Ok(Json(json!({
        "scope": task.clone().unwrap_or_else(|| "all".to_string()),
        "paused": paused,
    })))
}

async fn cancel(
    State(api): State<Api>,
    headers: HeaderMap,
    Path((project, name, run)): Path<(String, String, String)>,
) -> Result<Json<Value>, Failure> {
    authorise(&api, &headers)?;
    let id = format!("{project}/{name}");
    let mut daemon = api.daemon.lock().await;

    match daemon.cancel(&id, &run) {
        crate::daemon::CancelOutcome::Cancelled => {
            Ok(Json(json!({ "runId": run, "cancelled": true })))
        }
        crate::daemon::CancelOutcome::NotRunning => Err(Failure(
            StatusCode::CONFLICT,
            "not_running",
            format!("run {run:?} of {id:?} is not in flight"),
        )),
    }
}

/// Streams a Run's log as it is written.
async fn run_log_stream(
    State(api): State<Api>,
    headers: HeaderMap,
    Path((project, name, run)): Path<(String, String, String)>,
) -> Result<Response, Failure> {
    authorise(&api, &headers)?;
    let id = format!("{project}/{name}");
    let state_dir = api.daemon.lock().await.state_dir().clone();
    let path = crate::run::task_runs_dir(&state_dir, &id)
        .join(&run)
        .join(crate::run::RUN_LOG);
    if !path.exists() {
        return Err(Failure(
            StatusCode::NOT_FOUND,
            "no_such_run",
            format!("no log for run {run:?} of {id:?}"),
        ));
    }

    let stream = async_stream::stream! {
        let mut offset = 0u64;
        let mut settled = 0;
        loop {
            match tail(&path, offset) {
                Ok((chunk, next)) if !chunk.is_empty() => {
                    offset = next;
                    settled = 0;
                    yield Ok::<_, std::convert::Infallible>(
                        axum::response::sse::Event::default().data(chunk),
                    );
                }
                _ => {
                    // The Run is over once the record says so and nothing
                    // more has arrived; a couple of quiet passes avoids
                    // ending on a gap between writes.
                    let finished = crate::run::read_record(&state_dir, &id, &run)
                        .is_some_and(|record| record.status != crate::run::RunStatus::Running);
                    settled += 1;
                    if finished && settled > 2 {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                }
            }
        }
    };

    Ok(axum::response::Sse::new(stream).into_response())
}

/// Reads whatever has been appended since `offset`.
fn tail(path: &std::path::Path, offset: u64) -> std::io::Result<(String, u64)> {
    use std::io::{Read, Seek};
    let mut file = std::fs::File::open(path)?;
    file.seek(std::io::SeekFrom::Start(offset))?;
    let mut chunk = String::new();
    file.read_to_string(&mut chunk)?;
    let next = offset + chunk.len() as u64;
    Ok((chunk, next))
}
