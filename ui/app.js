// The whole client. No framework, no build step, no network beyond this
// daemon — everything below talks to /v1 with the session cookie.

const $ = (id) => document.getElementById(id);
const state = { tasks: [], showCompleted: false, current: null, stream: null };

async function api(path, options = {}) {
  const response = await fetch(path, { credentials: "same-origin", ...options });
  if (response.status === 401) {
    banner("This page is not signed in. Run `openroutine dashboard` to get a fresh link.");
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
  // A completed one-shot has no next moment, and the Status column already
  // says "completed" — repeating it here would be the same fact twice.
  if (task.completed) return "\u2014";
  if (!task.nextFireAt) return "no schedule";
  const at = new Date(task.nextFireAt);
  const shown = at.toLocaleString();
  return task.oneShot ? `${shown} (${countdown(task.nextFireAt)})` : shown;
}

function stateWord(text, kind) {
  const span = document.createElement("span");
  span.className = `state ${kind}`;
  span.textContent = text;
  return span;
}

function note(text) {
  const span = document.createElement("span");
  span.className = "note";
  span.textContent = text;
  return span;
}

/// Health as a word per line, plus the sentence that explains it.
///
/// The API sends `novelty` as "changed: the definition differs from the one
/// that last ran" — a whole sentence. The word before the colon belongs in the
/// Status column; the rest is an explanation, and explanations go under the
/// description with the parse error, not into a capsule beside the id.
function health(task) {
  const states = [];
  const notes = [];

  if (task.error) notes.push(task.error);
  if (task.broken) states.push(["broken", "broken"]);

  if (task.novelty) {
    // "new" and "changed" are the same condition to the scheduler — the
    // definition in front of it is not the one that last ran — so they share
    // one treatment and differ only in the word.
    const [word, rest] = splitOnce(task.novelty, ": ");
    states.push([word, "changed"]);
    if (rest) notes.push(task.novelty);
  }

  if (task.running) states.push(["running", "running"]);
  if (task.paused) states.push(["paused", "paused"]);
  if (task.completed) states.push(["completed", "completed"]);
  // Ready is a real state, not the absence of one: the definition parses and
  // names an agent that exists. Say so rather than leaving the cell blank.
  if (states.length === 0) states.push(["ready", "ready"]);

  return { states, notes };
}

function splitOnce(text, separator) {
  const at = text.indexOf(separator);
  return at === -1 ? [text, ""] : [text.slice(0, at), text.slice(at + separator.length)];
}

function renderTasks() {
  const body = document.querySelector("#tasks tbody");
  body.replaceChildren();

  const visible = state.tasks.filter((task) => state.showCompleted || !task.completed);
  $("empty").hidden = visible.length > 0;

  for (const task of visible) {
    const row = document.createElement("tr");

    const { states, notes } = health(task);

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

    const description = document.createElement("td");
    description.textContent = task.description ?? "—";
    description.append(...notes.map(note));

    const schedule = document.createElement("td");
    schedule.textContent = task.schedule ?? "—";

    const next = document.createElement("td");
    next.textContent = when(task);

    const status = document.createElement("td");
    status.append(...states.map(([text, kind]) => stateWord(text, kind)));

    row.append(id, description, schedule, next, status);
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
    // The same word-plus-colour treatment the task list uses, so "failed"
    // means the same thing to the eye in both tables. The word carries it;
    // the colour only agrees with the word.
    status.append(stateWord(run.status, run.status));
    if (run.exitCode !== undefined && run.exitCode !== null && run.status === "failed") {
      status.append(note(`exit ${run.exitCode}`));
    }

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
