// The whole client. No framework, no build step, no network beyond this
// daemon — everything below talks to /v1 with the session cookie.

const $ = (id) => document.getElementById(id);
const state = {
  tasks: [],
  showCompleted: false,
  current: null,
  stream: null,
  allPaused: false,
  taskPaused: false,
};

// ---- Language ----------------------------------------------------------
// English is the source of truth: it lives in the HTML and at every t()
// call site, and the dictionary holds only the Chinese. A key the
// dictionary lacks falls back to English rather than breaking the page.

const LANG_KEY = "or-lang";

function detectLocale() {
  let saved = null;
  try {
    saved = localStorage.getItem(LANG_KEY);
  } catch {
    // Storage can be walled off; the browser language still decides.
  }
  if (saved === "zh" || saved === "en") return saved;
  return (navigator.language ?? "").toLowerCase().startsWith("zh") ? "zh" : "en";
}

const locale = detectLocale();
if (locale === "zh") document.documentElement.lang = "zh-CN";

const MESSAGES = {
  zh: {
    // index.html, matched by data-i18n.
    "skip": "跳到正文",
    "tagline": "你的 AI 智能体的 crontab，以 Markdown 写成。本页的一切都读取自这台机器上的文件。",
    "tasks.heading": "任务",
    "show-completed": "显示已完成",
    "tasks.caption": "所有已注册的任务，以及各自调度下一次到期的时刻。",
    "th.task": "任务",
    "th.description": "描述",
    "th.schedule": "调度",
    "th.next": "下次",
    "th.status": "状态",
    "empty": "还没有任务。使用 {command} 注册一个任务文件。",
    "back": "← 全部任务",
    "run-now": "立即运行",
    "runs.heading": "运行记录",
    "runs.caption": "此任务保存在磁盘上的所有运行记录，最新的在前。",
    "th.started": "开始时间",
    "th.trigger": "触发方式",
    "th.actions": "操作",
    "no-runs": "还没有任何运行。",
    "footer": "任务以文件的方式编辑。本页从不写入任何文件。",
    // The rest of this file.
    "signin": "本页尚未登录。运行 `openroutine dashboard` 获取新的链接。",
    "request-failed": "请求失败（{n}）",
    "due": "已到期",
    "no-schedule": "无调度",
    "state.broken": "损坏",
    "state.running": "运行中",
    "state.paused": "已暂停",
    "state.completed": "已完成",
    "state.ready": "就绪",
    "state.new": "新",
    "state.changed": "已变更",
    "note.new": "新：此任务从未运行过",
    "note.changed": "已变更：当前定义与上次运行的版本不同",
    "everything-paused": "已全部暂停。恢复之前不会有任何任务触发。",
    "pause-everything": "全部暂停",
    "resume-everything": "全部恢复",
    "edit-hint": "{path} — 修改任务请编辑此文件",
    "pause": "暂停",
    "resume": "恢复",
    "trigger.schedule": "定时",
    "trigger.fire": "手动",
    "run.running": "运行中",
    "run.succeeded": "成功",
    "run.failed": "失败",
    "run.timed-out": "超时",
    "run.interrupted": "已中断",
    "exit": "退出码 {n}",
    "log": "日志",
    "cancel": "取消",
  },
};

function t(key, english) {
  return locale === "zh" ? MESSAGES.zh[key] ?? english : english;
}

function fmt(template, values) {
  return template.replace(/\{(\w+)\}/g, (match, name) => values[name] ?? match);
}

async function api(path, options = {}) {
  const response = await fetch(path, { credentials: "same-origin", ...options });
  if (response.status === 401) {
    banner(t("signin", "This page is not signed in. Run `openroutine dashboard` to get a fresh link."));
    throw new Error("unauthorized");
  }
  const body = await response.json().catch(() => ({}));
  if (!response.ok) {
    throw new Error(
      body?.error?.message ?? fmt(t("request-failed", "request failed ({n})"), { n: response.status }),
    );
  }
  return body;
}

function banner(message) {
  const element = $("banner");
  element.textContent = message;
  element.hidden = !message;
}

/// A one-shot's moment, written as the time remaining.
const relative = new Intl.RelativeTimeFormat(locale === "zh" ? "zh-CN" : "en", { style: "short" });

function countdown(iso) {
  const remaining = new Date(iso) - new Date();
  if (remaining <= 0) return t("due", "due");
  const minutes = Math.round(remaining / 60000);
  if (minutes < 60) return relative.format(minutes, "minute");
  const hours = Math.round(minutes / 60);
  if (hours < 48) return relative.format(hours, "hour");
  return relative.format(Math.round(hours / 24), "day");
}

// An explicit locale keeps the manual language switch in charge of the
// date format too; under English the browser's regional habits hold.
function showTime(date) {
  return date.toLocaleString(locale === "zh" ? "zh-CN" : undefined);
}

function when(task) {
  // A completed one-shot has no next moment, and the Status column already
  // says "completed" — repeating it here would be the same fact twice.
  if (task.completed) return "\u2014";
  if (!task.nextFireAt) return t("no-schedule", "no schedule");
  const at = new Date(task.nextFireAt);
  const shown = showTime(at);
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
  if (task.broken) states.push([t("state.broken", "broken"), "broken"]);

  if (task.novelty) {
    // "new" and "changed" are the same condition to the scheduler — the
    // definition in front of it is not the one that last ran — so they share
    // one treatment and differ only in the word. A sentence the NOVELTY map
    // does not know keeps the old split-on-colon treatment, untranslated.
    const known = NOVELTY.get(task.novelty);
    const [word, rest] = splitOnce(task.novelty, ": ");
    states.push([known ? t(...known.word) : word, "changed"]);
    if (rest) notes.push(known ? t(known.note, task.novelty) : task.novelty);
  }

  if (task.running) states.push([t("state.running", "running"), "running"]);
  if (task.paused) states.push([t("state.paused", "paused"), "paused"]);
  if (task.completed) states.push([t("state.completed", "completed"), "completed"]);
  // Ready is a real state, not the absence of one: the definition parses and
  // names an agent that exists. Say so rather than leaving the cell blank.
  if (states.length === 0) states.push([t("state.ready", "ready"), "ready"]);

  return { states, notes };
}

function splitOnce(text, separator) {
  const at = text.indexOf(separator);
  return at === -1 ? [text, ""] : [text.slice(0, at), text.slice(at + separator.length)];
}

// The exact sentences Novelty::note() in src/registry.rs can send, mapped to
// the word the Status column shows and the note under the description. Both
// files must change together.
const NOVELTY = new Map([
  ["new: this task has never run", { word: ["state.new", "new"], note: "note.new" }],
  [
    "changed: the definition differs from the one that last ran",
    { word: ["state.changed", "changed"], note: "note.changed" },
  ],
]);

// Raw API words shown to the reader. The raw value always stays the CSS
// class and the wire value; only what the cell says goes through here.
function displayRunStatus(status) {
  return t(`run.${status}`, status);
}

function displayTrigger(trigger) {
  return t(`trigger.${trigger}`, trigger);
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
  state.allPaused = Boolean(body.paused);
  banner(body.paused ? t("everything-paused", "Everything is paused. Nothing will fire until you resume.") : "");
  $("pause-all").textContent = body.paused
    ? t("resume-everything", "Resume everything")
    : t("pause-everything", "Pause everything");
  renderTasks();
}

async function openTask(id) {
  state.current = id;
  $("tasks-view").hidden = true;
  $("task-view").hidden = false;
  stopStream();

  const task = await api(`/v1/tasks/${encodeURIComponent(id)}`);
  $("task-title").textContent = id;
  $("task-path").textContent = fmt(t("edit-hint", "{path} — edit this file to change the task"), {
    path: task.path ?? "",
  });
  state.taskPaused = Boolean(task.paused);
  $("pause-task").textContent = task.paused ? t("resume", "Resume") : t("pause", "Pause");
  $("run-now").disabled = Boolean(task.running);

  const history = await api(`/v1/tasks/${encodeURIComponent(id)}/runs`);
  const body = document.querySelector("#runs tbody");
  body.replaceChildren();
  $("no-runs").hidden = history.runs.length > 0;

  for (const run of history.runs) {
    const row = document.createElement("tr");
    const started = document.createElement("td");
    started.textContent = showTime(new Date(run.startedAt));
    const trigger = document.createElement("td");
    trigger.textContent = displayTrigger(run.trigger);
    const status = document.createElement("td");
    // The same word-plus-colour treatment the task list uses, so "failed"
    // means the same thing to the eye in both tables. The word carries it;
    // the colour only agrees with the word.
    status.append(stateWord(displayRunStatus(run.status), run.status));
    if (run.exitCode !== undefined && run.exitCode !== null && run.status === "failed") {
      status.append(note(fmt(t("exit", "exit {n}"), { n: run.exitCode })));
    }

    const actions = document.createElement("td");
    const view = document.createElement("button");
    view.className = "ghost";
    view.textContent = t("log", "log");
    view.addEventListener("click", () => showLog(id, run));
    actions.append(view);

    if (run.status === "running") {
      const stop = document.createElement("button");
      stop.className = "ghost";
      stop.textContent = t("cancel", "cancel");
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
  await api(state.allPaused ? "/v1/resume" : "/v1/pause", { method: "POST" });
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
  const action = state.taskPaused ? "resume" : "pause";
  await api(`/v1/tasks/${encodeURIComponent(state.current)}/${action}`, { method: "POST" });
  openTask(state.current);
});

// The HTML is written in English; under Chinese every [data-i18n] element is
// re-worded in place before the first fetch. The one element that embeds a
// <code> child names that spot with {command}, and the sentence is rebuilt
// around the untranslated command.
function applyStaticTranslations() {
  if (locale !== "zh") return;
  for (const element of document.querySelectorAll("[data-i18n]")) {
    const message = MESSAGES.zh[element.dataset.i18n];
    if (message === undefined) continue;
    const code = element.querySelector("code");
    if (code && message.includes("{command}")) {
      const [before, after] = message.split("{command}");
      element.replaceChildren(before, code, after);
    } else {
      element.textContent = message;
    }
  }
}

// The switch names the language it would take you to, in that language.
function initLanguageSwitch() {
  const other = locale === "zh" ? "en" : "zh";
  const button = $("lang");
  button.textContent = other === "zh" ? "中文" : "English";
  button.lang = other === "zh" ? "zh-CN" : "en";
  button.hidden = false;
  button.addEventListener("click", () => {
    try {
      localStorage.setItem(LANG_KEY, other);
    } catch {
      // Nowhere to remember the choice; the reload falls back to the
      // browser language.
    }
    location.reload();
  });
}

applyStaticTranslations();
initLanguageSwitch();
refresh().catch((error) => banner(error.message));
setInterval(() => {
  if (!$("tasks-view").hidden) refresh().catch(() => {});
}, 5000);
