// The whole client. No framework, no build step, no network beyond this
// daemon — everything below talks to /v1 with the session cookie.

const $ = (id) => document.getElementById(id);
const state = { tasks: [], showCompleted: false, current: null, stream: null };

async function api(path, options = {}) {
  const response = await fetch(path, { credentials: "same-origin", ...options });
  if (response.status === 401) {
    banner("This page is not signed in. Run `openroutine open` to get a fresh link.");
    throw new Error("unauthorized");
  }
  const body = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(body?.error?.message ?? `request failed (${response.status})`);
  }
  return body;
}

function banner(message) {
  const element = $("banner");
  element.textContent = message;
  element.hidden = !message;
}

/// A one-shot's moment, written as the time remaining.
function countdown(iso) {
  const remaining = new Date(iso) - new Date();
  if (remaining <= 0) return "due";
  const minutes = Math.round(remaining / 60000);
  if (minutes < 60) return `in ${minutes} min`;
  const hours = Math.round(minutes / 60);
  if (hours < 48) return `in ${hours} h`;
  return `in ${Math.round(hours / 24)} days`;
}

function when(task) {
  if (task.completed) return "completed";
  if (!task.nextFireAt) return "no schedule";
  const at = new Date(task.nextFireAt);
  const shown = at.toLocaleString();
  return task.oneShot ? `${shown} (${countdown(task.nextFireAt)})` : shown;
}

function tag(text, kind) {
  const span = document.createElement("span");
  span.className = `tag ${kind}`;
  span.textContent = text;
  return span;
}

function renderTasks() {
  const body = document.querySelector("#tasks tbody");
  body.replaceChildren();

  const visible = state.tasks.filter((task) => state.showCompleted || !task.completed);
  $("empty").hidden = visible.length > 0;

  for (const task of visible) {
    const row = document.createElement("tr");

    const id = document.createElement("td");
    id.className = "id";
    const link = document.createElement("a");
    link.href = "#";
    link.textContent = task.id;
    link.addEventListener("click", (event) => {
      event.preventDefault();
      openTask(task.id);
    });
    id.append(link);
    if (task.broken) id.append(tag("broken", "broken"));
    if (task.novelty) id.append(tag(task.novelty, "new"));
    if (task.running) id.append(tag("running", "running"));
    if (task.paused) id.append(tag("paused", "paused"));

    const description = document.createElement("td");
    description.textContent = task.description ?? "—";
    if (task.error) {
      const note = document.createElement("span");
      note.className = "note";
      note.textContent = task.error;
      description.append(note);
    }

    const schedule = document.createElement("td");
    schedule.textContent = task.schedule ?? "—";

    const next = document.createElement("td");
    next.textContent = when(task);

    row.append(id, description, schedule, next, document.createElement("td"));
    body.append(row);
  }
}

async function refresh() {
  const body = await api("/v1/tasks");
  state.tasks = body.tasks;
  banner(body.paused ? "Everything is paused. Nothing will fire until you resume." : "");
  $("pause-all").textContent = body.paused ? "Resume everything" : "Pause everything";
  renderTasks();
}

async function openTask(id) {
  state.current = id;
  $("tasks-view").hidden = true;
  $("task-view").hidden = false;
  stopStream();

  const task = await api(`/v1/tasks/${encodeURIComponent(id)}`);
  $("task-title").textContent = id;
  $("task-path").textContent = `${task.path ?? ""} — edit this file to change the task`;
  $("pause-task").textContent = task.paused ? "Resume" : "Pause";
  $("run-now").disabled = Boolean(task.running);

  const history = await api(`/v1/tasks/${encodeURIComponent(id)}/runs`);
  const body = document.querySelector("#runs tbody");
  body.replaceChildren();
  $("no-runs").hidden = history.runs.length > 0;

  for (const run of history.runs) {
    const row = document.createElement("tr");
    const started = document.createElement("td");
    started.textContent = new Date(run.startedAt).toLocaleString();
    const trigger = document.createElement("td");
    trigger.textContent = run.trigger;
    const status = document.createElement("td");
    status.textContent = run.status;

    const actions = document.createElement("td");
    const view = document.createElement("button");
    view.className = "ghost";
    view.textContent = "log";
    view.addEventListener("click", () => showLog(id, run));
    actions.append(view);

    if (run.status === "running") {
      const stop = document.createElement("button");
      stop.className = "ghost";
      stop.textContent = "cancel";
      stop.addEventListener("click", async () => {
        await api(`/v1/runs/${encodeURIComponent(id)}/${run.runId}/cancel`, { method: "POST" });
        openTask(id);
      });
      actions.append(stop);
    }

    row.append(started, trigger, status, actions);
    body.append(row);
  }
}

async function showLog(id, run) {
  const log = $("log");
  log.hidden = false;
  log.textContent = "";
  stopStream();

  if (run.status === "running") {
    // Follow it as it is written; the daemon closes the stream when the
    // run ends.
    state.stream = new EventSource(`/v1/runs/${encodeURIComponent(id)}/${run.runId}/log/stream`);
    state.stream.onmessage = (event) => {
      log.textContent += event.data;
      log.scrollTop = log.scrollHeight;
    };
    state.stream.onerror = () => stopStream();
    return;
  }

  const response = await fetch(`/v1/runs/${encodeURIComponent(id)}/${run.runId}/log`, {
    credentials: "same-origin",
  });
  log.textContent = await response.text();
}

function stopStream() {
  state.stream?.close();
  state.stream = null;
}

$("back").addEventListener("click", () => {
  stopStream();
  $("task-view").hidden = true;
  $("tasks-view").hidden = false;
  refresh();
});

$("show-completed").addEventListener("change", (event) => {
  state.showCompleted = event.target.checked;
  renderTasks();
});

$("pause-all").addEventListener("click", async () => {
  const paused = $("pause-all").textContent.startsWith("Pause");
  await api(paused ? "/v1/pause" : "/v1/resume", { method: "POST" });
  refresh();
});

$("run-now").addEventListener("click", async () => {
  try {
    await api(`/v1/tasks/${encodeURIComponent(state.current)}/fire`, { method: "POST" });
  } catch (error) {
    banner(error.message);
  }
  openTask(state.current);
});

$("pause-task").addEventListener("click", async () => {
  const paused = $("pause-task").textContent === "Pause";
  await api(`/v1/tasks/${encodeURIComponent(state.current)}/${paused ? "pause" : "resume"}`, { method: "POST" });
  openTask(state.current);
});

refresh().catch((error) => banner(error.message));
setInterval(() => {
  if (!$("tasks-view").hidden) refresh().catch(() => {});
}, 5000);
